"""jina-reranker-v3.5 behind llama-server's /v1/rerank API.

llama.cpp cannot run this model yet (ggml-org/llama.cpp#26286), so this serves
it with transformers on the GPU. InferMux starts it as a model like any
llama-server, with --port, and waits for /health. See docs/decisions.md,
"Reranking with jina-reranker-v3.5".

The model's own modeling.py is imported from the model directory rather than
through trust_remote_code, so the code that runs is the file pinned next to
the weights, and nothing is written to a module cache.

Request:  {"query": str, "documents": [str | {"text": str}], "top_n": int?}
Response: {"model", "object": "list", "usage": {...},
           "results": [{"index": int, "relevance_score": float}]}, best first.
"""

import argparse
import importlib.util
import json
import os
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
# Triton compiles kernels on first use into ~/.triton. InferMux gives its
# models a writable HOME; anywhere else without one, a temporary directory.
if not os.access(os.path.expanduser("~"), os.W_OK):
    os.environ.setdefault("TRITON_CACHE_DIR", tempfile.mkdtemp(prefix="semsearch-rerank-triton-"))

import torch  # noqa: E402


def load(model_dir: Path, device: str):
    spec = importlib.util.spec_from_file_location("jina_modeling", model_dir / "modeling.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    dtype = torch.bfloat16 if device == "cuda" else torch.float32
    return module.JinaForRanking.from_pretrained(str(model_dir), dtype=dtype).to(device).eval()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--model", type=Path, required=True, help="directory with the weights, config, tokenizer and modeling.py")
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--device", default="cuda")
    # 30 code pieces were 26-40k characters and peaked at 2.5 GB of VRAM.
    p.add_argument("--max-chars", type=int, default=120_000, help="refuse a request whose query and documents are longer")
    args = p.parse_args()

    name = args.model.name
    t = time.perf_counter()
    model = load(args.model, args.device)
    print(f"loaded {name} on {args.device} in {time.perf_counter() - t:.1f} s", file=sys.stderr, flush=True)
    gpu = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        def reply(self, status, body):
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/health":
                self.reply(200, {"status": "ok"})
            elif self.path == "/v1/models":
                self.reply(200, {"object": "list", "data": [{"id": name, "object": "model"}]})
            else:
                self.reply(404, {"error": {"message": f"no route {self.path}"}})

        def do_POST(self):
            if self.path not in ("/v1/rerank", "/rerank", "/v1/reranking", "/reranking"):
                return self.reply(404, {"error": {"message": f"no route {self.path}"}})
            try:
                req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
                query = req["query"]
                docs = [d["text"] if isinstance(d, dict) else d for d in req["documents"]]
                if not isinstance(query, str) or not all(isinstance(d, str) for d in docs):
                    raise TypeError("query and documents must be text")
            except (ValueError, KeyError, TypeError) as e:
                return self.reply(400, {"error": {"message": f"bad request: {e}"}})
            chars = len(query) + sum(map(len, docs))
            if chars > args.max_chars:
                return self.reply(413, {"error": {"message": f"{chars} characters, the limit is {args.max_chars}"}})
            if not docs:
                return self.reply(200, {"model": name, "object": "list", "usage": {}, "results": []})

            t = time.perf_counter()
            try:
                with gpu, torch.inference_mode():
                    ranked = model.rerank(query, docs, top_n=req.get("top_n"))
            except Exception as e:
                print(f"rerank failed: {e!r}", file=sys.stderr, flush=True)
                return self.reply(500, {"error": {"message": f"rerank failed: {e}"}})
            dt = time.perf_counter() - t
            print(f"rerank {len(docs)} documents, {chars} characters, {dt:.2f} s", file=sys.stderr, flush=True)
            self.reply(200, {
                "model": name,
                "object": "list",
                "usage": {"documents": len(docs), "characters": chars},
                "results": [{"index": int(r["index"]), "relevance_score": float(r["relevance_score"])} for r in ranked],
            })

        def log_message(self, *_):
            pass

    ThreadingHTTPServer((args.host, args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
