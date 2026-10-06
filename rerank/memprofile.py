"""Peak VRAM of one rerank of 30 pieces of this repository's code, with the
model's own forward or the server's, and what is live at the peak.

    <reranker's python> rerank/memprofile.py original|fixed|flash <characters per piece> <weights dir>

original is the model's own forward with SDPA, fixed the server's forward with
SDPA, flash the server's forward with flash-attn, as the server runs it.

Run it with the interpreter InferMux runs server.py with, on a card the live
reranker has been unloaded from (`/warden/unload`). It writes the scores to
/tmp/rerank-profile-<variant>-<size>.json, to compare the two forwards.
"""
import os, sys, random, json, collections
from pathlib import Path
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
os.environ.setdefault("TORCH_DISABLE_NATIVE_JIT", "1")
import torch, server

variant, size, M = sys.argv[1], int(sys.argv[2]), Path(sys.argv[3])
mod = server.modeling(M)
model = server.load(M, "cuda", mod.JinaForRanking if variant == "original" else server.ranker(mod), "flash_attention_2" if variant == "flash" else "sdpa")
print("attn implementation:", model.config._attn_implementation)

random.seed(1)
files = sorted(HERE.parent.glob("src/*.rs")) + sorted(HERE.parent.glob("bench/*.py")) + sorted(HERE.parent.glob("vscode/**/*.js"))
text = "".join(f.read_text() for f in files)
docs = [text[i:i + size] for i in random.sample(range(len(text) - size), 30)]
query = "where do we pick the next failover host"
model._ensure_tokenizer()
print("tokens:", sum(len(model._tokenizer(d)["input_ids"]) for d in docs), "chars:", sum(map(len, docs)))

masks = []
model.model.layers[0].self_attn.register_forward_pre_hook(lambda m, a, kw: masks.append(None if kw.get("attention_mask") is None else (tuple(kw["attention_mask"].shape), str(kw["attention_mask"].dtype))), with_kwargs=True)

import statistics, time
with torch.inference_mode():
    model.rerank(query, docs)  # warm-up: kernels, cuBLAS handles
    times = []
    for _ in range(5):
        torch.cuda.synchronize(); t = time.perf_counter()
        model.rerank(query, docs)
        torch.cuda.synchronize(); times.append(time.perf_counter() - t)
print(f"rerank time, median of 5 after a warm-up: {statistics.median(times):.3f} s (min {min(times):.3f}, max {max(times):.3f})")
torch.cuda.empty_cache()

torch.cuda.synchronize(); base = torch.cuda.memory_allocated()
torch.cuda.memory._record_memory_history(max_entries=200000)
torch.cuda.reset_peak_memory_stats()
with torch.inference_mode():
    ranked = model.rerank(query, docs)
torch.cuda.synchronize()
peak, reserved = torch.cuda.max_memory_allocated(), torch.cuda.max_memory_reserved()
snap = torch.cuda.memory._snapshot()
torch.cuda.memory._record_memory_history(enabled=None)
print("attention mask at layer 0:", masks[:2])
print(f"weights and buffers: {base / 2**20:.0f} MiB, peak allocated: {peak / 2**20:.0f} MiB (+{(peak - base) / 2**20:.0f}), peak reserved: {reserved / 2**20:.0f} MiB")

# Replay alloc/free events to find what was live at the peak.
live, total, best, at_best = {}, 0, 0, {}
for ev in snap["device_traces"][0]:
    if ev["action"] == "alloc":
        live[ev["addr"]] = ev; total += ev["size"]
        if total > best: best, at_best = total, dict(live)
    elif ev["action"] in ("free_requested", "free_completed") and ev["addr"] in live and ev["action"] == "free_completed":
        total -= live.pop(ev["addr"])["size"]
def where(ev):
    frames = [f for f in ev.get("frames", []) if f["filename"].endswith(".py") and "torch/" not in f["filename"] and "prof.py" not in f["filename"]]
    f = frames[0] if frames else (ev.get("frames") or [{"filename": "?", "line": 0, "name": "?"}])[0]
    return f"{Path(f['filename']).name}:{f['line']} {f['name']}"
groups = collections.defaultdict(lambda: [0, 0])
for ev in at_best.values():
    g = groups[where(ev)]; g[0] += ev["size"]; g[1] += 1
print(f"live at the peak, allocated during the rerank: {best / 2**20:.0f} MiB")
for k, (b, n) in sorted(groups.items(), key=lambda kv: -kv[1][0])[:12]:
    print(f"  {b / 2**20:8.1f} MiB  {n:4d} tensors  {k}")
json.dump([[int(r["index"]), float(r["relevance_score"])] for r in ranked], open(f"/tmp/rerank-profile-{variant}-{size}.json", "w"))
