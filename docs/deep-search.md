# Deep search

A small local agent that searches on the user's behalf, beside the instant search. Design stage, nothing is built. Started 2026-10-06.

## What the user decided (2026-10-06)

- The search widget sits in the center of the screen, slightly above the middle.
- The instant results (embed, rerank, one item per file) appear below the input, as they do now. They stay fast and predictable.
- The deep search is one row above the input. Pressing Up from the input selects it, and selecting it starts the deep search for the current text.
- The agent chooses which results to show and shows only the best ones. It may run more searches to refine them and read the files the results point to, to check that they answer the question.
- It is a small agent with tools, not a one-pass explainer. Each result it keeps gets a line on why it answers the question or why it is relevant. Simple follow-up questions were mentioned as a possibility.
- Git history is in scope later: commits, branches, worktrees and other history.

## Why

The instant list is what a user expects from a search box: the same text gives the same list, at once. The agent costs seconds and can be wrong, so it is something the user asks for, not something that replaces the list.

## Open

- **UI.** VS Code's quick pick only lists items below its input and sits at the top of the window, so this layout likely needs a webview, unless the host can place a quick pick and show an item above the input. Not probed.
- **Where the loop runs.** Recommended: in semsearch (`semsearch ask`, JSON events), so the CLI and agents get it too.
- **Model, and whether to fine-tune.** Handed to a separate session. Candidates and constraints: see the handover in the session of 2026-10-06, and the research below.
- **The reranker inside the agent.** The 3080 holds one model at a time, so a rerank per agent search would evict the agent's model. Recommended: the agent's searches skip the reranker; instant search keeps it.
- **Time budget.** Not decided.
- **Follow-up questions.** Mentioned, not decided.

## Research, 2026-10-06

No open model found is trained to write a short, user-facing line per hit on why it is relevant. The nearest:

- rank1 (jhu-clsp, 0.5B-32B, MIT, Qwen2.5): `<think>` then true/false per document. Pointwise, long reasoning.
- ReasonRank-7B and REARANK-7B (MIT, Qwen2.5): one reasoning trace over a list, then an order.
- ExaRanker (2023, T5): a relevance label and a short explanation on demand. The right output, trained on English web passages.
- Verbal-R3 (arXiv 2605.01399): verbal annotations of relevance; no weights found.
- Code-tuned scoring rerankers: CoREB-Reranker (arXiv 2605.04615), KaLM-Reranker-V1, a Qwen3-Reranker-4B tuned on code.

None of these does tool calling, which the agent needs.
