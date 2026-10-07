"""The agent's four tools (agent.py: search, grep, read, git) as an MCP server
over stdio, so a Claude Code session can run the agent with exactly them.
See bench/README.md, "Agent".

Started by agent.py's --claude mode through --mcp-config. Each call appends a
line to $BENCH_STATS with what agent.py counts per run.
"""

import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import agent  # noqa: E402


def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    msg |= {"error": error} if error else {"result": result}
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def main():
    tools = [{"name": t["function"]["name"], "description": t["function"]["description"],
              "inputSchema": t["function"]["parameters"]} for t in agent.TOOLS]
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        method, id_ = msg.get("method"), msg.get("id")
        if id_ is None:  # a notification
            continue
        if method == "initialize":
            reply(id_, {"protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
                        "capabilities": {"tools": {}}, "serverInfo": {"name": "bench", "version": "1"}})
        elif method == "tools/list":
            reply(id_, {"tools": tools})
        elif method == "tools/call":
            name, args = msg["params"]["name"], msg["params"].get("arguments") or {}
            stats = {"degraded_searches": 0, "bad_calls": 0}
            try:
                out = agent.call(name, args, stats)
            except Exception as e:
                stats["bad_calls"] += 1
                out = f"error: {type(e).__name__}: {e}"
            if out.startswith("error: no tool") or out.startswith("error: allowed"):
                stats["bad_calls"] += 1
            with open(os.environ["BENCH_STATS"], "a") as f:
                f.write(json.dumps({"tool": name} | stats) + "\n")
            reply(id_, {"content": [{"type": "text", "text": out}], "isError": out.startswith("error:")})
        elif method == "ping":
            reply(id_, {})
        else:
            reply(id_, error={"code": -32601, "message": f"no method {method}"})


if __name__ == "__main__":
    main()
