# /// script
# requires-python = ">=3.12"
# ///
"""Speed and VRAM of candidate agent models on reaperboi's card, with llama-bench.
See bench/README.md.

    uv run bench/llm_speed.py /srv/models/agent-candidates/*.gguf --note "..."

Each model gets prompt processing (pp512) and generation (tg128) at context
depths 0, 8192 and 16384, since the agent's context grows with every tool
result. VRAM is the peak nvidia-smi attributes to llama-bench's process. A
foreign process on the card makes InferMux's warden unload its models, so the
first seconds may overlap with an unload; one warm-up run absorbs that.

Writes bench/results/<time>-llm-speed.json and prints a summary.
"""

import argparse
import json
import subprocess
import sys
import threading
import time
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
LLAMA = Path("/nix/store/0cw8p4njzwbzf2032fhcx9f9gf7r38sc-llama-cpp-0.5.0/bin")
DEPTHS = "0,8192,16384"


def smi(*query: str) -> list[list[str]]:
    out = subprocess.run(
        ["nvidia-smi", f"--query-{query[0]}={','.join(query[1:])}", "--format=csv,noheader,nounits"],
        capture_output=True, text=True, check=True,
    ).stdout
    return [[c.strip() for c in line.split(",")] for line in out.strip().splitlines()]


def peak_vram(proc: subprocess.Popen, peak: list[int]) -> None:
    while proc.poll() is None:
        for pid, mib in smi("compute-apps", "pid", "used_memory"):
            if pid == str(proc.pid):
                peak[0] = max(peak[0], int(mib))
        time.sleep(0.2)


def bench(model: Path, reps: int) -> dict:
    cmd = [
        str(LLAMA / "llama-bench"), "-m", str(model), "-ngl", "99", "-fa", "1",
        "-p", "512", "-n", "128", "-d", DEPTHS, "-r", str(reps), "-o", "json",
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    peak = [0]
    watcher = threading.Thread(target=peak_vram, args=(proc, peak))
    watcher.start()
    out, err = proc.communicate()
    watcher.join()
    if proc.returncode:
        return {"model": model.name, "error": err.strip().splitlines()[-5:]}
    rows = json.loads(out)
    tests = [
        {
            "test": "pp" if r["n_prompt"] else "tg",
            "depth": r["n_depth"],
            "tok_s": round(r["avg_ts"], 1),
            "stddev": round(r["stddev_ts"], 1),
        }
        for r in rows
    ]
    return {
        "model": model.name,
        "bytes": model.stat().st_size,
        "params": rows[0]["model_n_params"],
        "type": rows[0]["model_type"],
        "peak_vram_mib": peak[0],
        "tests": tests,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("models", nargs="+", type=Path)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--note", default="")
    args = ap.parse_args()

    gpu = smi("gpu", "name", "driver_version", "memory.total")[0]
    build = subprocess.run([str(LLAMA / "llama-bench"), "--version"], capture_output=True, text=True)
    result = {
        "note": args.note,
        "gpu": gpu,
        "llama_cpp": (build.stdout + build.stderr).strip().splitlines()[0],
        "flags": f"-ngl 99 -fa 1 -p 512 -n 128 -d {DEPTHS} -r {args.reps}, f16 KV cache",
        "others_on_card": smi("compute-apps", "pid", "process_name", "used_memory"),
        "models": [],
    }
    bench(args.models[0], 1)  # warm-up while the warden unloads InferMux's models
    for model in args.models:
        r = bench(model, args.reps)
        result["models"].append(r)
        if "error" in r:
            print(f"{model.name}: FAILED {r['error']}", file=sys.stderr)
            continue
        cells = {(t["test"], t["depth"]): t["tok_s"] for t in r["tests"]}
        print(
            f"{model.name:48} {r['params'] / 1e9:5.2f}B {r['peak_vram_mib']:6} MiB  "
            f"pp {cells[('pp', 0)]:7} / {cells[('pp', 16384)]:7}  "
            f"tg {cells[('tg', 0)]:6} / {cells[('tg', 8192)]:6} / {cells[('tg', 16384)]:6}"
        )
    out = HERE / "results" / f"{datetime.now():%Y-%m-%d-%H%M%S}-llm-speed.json"
    out.write_text(json.dumps(result, indent=2) + "\n")
    print(out)


if __name__ == "__main__":
    main()
