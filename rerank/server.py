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
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
# torch routes some eager ops, the rotary embedding's fp32 matmul among them,
# to Triton kernels it compiles into ~/.triton and loads from there. InferMux's
# cache directory, its models' HOME, is mounted noexec, so the load fails.
# Without them the scores are identical and as fast (2026-10-06).
os.environ.setdefault("TORCH_DISABLE_NATIVE_JIT", "1")

import torch  # noqa: E402


def modeling(model_dir: Path):
    spec = importlib.util.spec_from_file_location("jina_modeling", model_dir / "modeling.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def ranker(module):
    """The model's JinaForRanking with a forward that keeps only the last
    layer's hidden states. The original asks for all 29 layers' and uses the
    last, which held about 1 GB of VRAM for 30 pieces of 2,000 characters.
    The scores are the same computation, so the same numbers."""

    class Ranker(module.JinaForRanking):
        def forward(self, input_ids, attention_mask=None, **_):
            hidden = self.model(input_ids=input_ids, attention_mask=attention_mask, use_cache=False).last_hidden_state
            batch, _, dim = hidden.shape
            docs = self.projector(hidden[input_ids == self.doc_embed_token_id].view(batch, -1, dim))
            query = self.projector(hidden[input_ids == self.query_embed_token_id].unsqueeze(1))
            scores = torch.nn.functional.cosine_similarity(docs, query.expand_as(docs), dim=-1).squeeze(-1)
            return module.CausalLMOutputWithScores(scores=scores, query_embeds=query, doc_embeds=docs)

    return Ranker


def load(model_dir: Path, device: str, cls=None, attention=None):
    """On the GPU, attention runs through flash-attn. 16 of the model's 28
    layers attend to a 1,024-token window; flash-attn skips what lies outside
    it, where SDPA built an n x n mask, 1.4 GB for 16k tokens. Without
    flash-attn this fails rather than fall back to that."""
    cls = cls or ranker(modeling(model_dir))
    dtype = torch.bfloat16 if device == "cuda" else torch.float32
    attention = attention or ("flash_attention_2" if device == "cuda" else "sdpa")
    return cls.from_pretrained(str(model_dir), dtype=dtype, attn_implementation=attention).to(device).eval()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--model", type=Path, required=True, help="directory with the weights, config, tokenizer and modeling.py")
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--device", default="cuda")
    # 30 pieces of 2,000 characters, hister's limit, are 60k characters and
    # peak at 1.8 GB of VRAM with flash-attn.
    p.add_argument("--max-chars", type=int, default=120_000, help="refuse a request whose query and documents are longer")
    args = p.parse_args()

    name = args.model.name
    t = time.perf_counter()
    model = load(args.model, args.device)
    print(f"loaded {name} on {args.device} with {model.config._attn_implementation} in {time.perf_counter() - t:.1f} s", file=sys.stderr, flush=True)
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
