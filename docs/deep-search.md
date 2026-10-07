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
- **Model, and whether to fine-tune.** In progress in a separate session. The reranker and the agent's model either share the 3080 (about 4 GB left for the model) or the model takes over the reranker's job (about 8.5 GB). Fine-tuning is not decided; where its training data may come from is under "Training data and terms of use" below.
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

## Training data and terms of use, 2026-10-06

The user reaches Opus, Fable, Sol and Astra through subscriptions, so the consumer terms apply, not the API agreements.

- Anthropic's [Consumer Terms](https://www.anthropic.com/legal/consumer-terms) §3(2) forbid using the services "to develop or train any artificial intelligence or machine learning algorithms or models". Read literally, that covers any model, competing or not, and has no approval clause.
- OpenAI's [Terms of Use](https://openai.com/policies/row-terms-of-use/) forbid "Using Output to develop models that compete with OpenAI". Whether a private code-search model competes is not defined.
- The API agreements are narrower (Anthropic's [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) §D.4 and the [OpenAI Services Agreement](https://cdn.openai.com/osa/openai-services-agreement.pdf) §(e) both say "competing"), but are not in use.

Current plan (the user, 2026-10-06):

- Sol and Astra may generate training data: questions, trajectories, judgments that select trajectories. This relies on the "compete" reading; the risk accepted is the account, not a lawsuit.
- Opus and Fable only evaluate: judging answers to `bench/queries.toml`, grading why-lines, writing a held-out set. None of that trains a model.
- Under consideration: about $100 of OpenRouter credit for an open-weight teacher. MIT-licensed weights carry no restriction on using outputs. Candidates as of 2026-10-06: GLM-5.3-Flash (321B, MIT, $0.15/$0.50 per M tokens) and DeepSeek-V4.1-Flash (763B, MIT, about $0.15/$0.60). Community fine-tunes trained on Claude output (Qwopus, Qwythos) are not teachers, since that output falls under the clause above.
- The `bench/queries.toml` questions are never training data.

## Agent eval, 2026-10-07

`bench/agent.py` on the 26 queries, 3 runs each, temperature 0.6; results in `bench/results/*-agent.json`. "Found" is the share of runs where a shown hit is correct, under the lenient reading (bench/README.md). Instant search scored the same way: top-3 0.62 found at precision 0.28, top-5 0.73 at 0.21.

| Model | Thinking budget | Found | Precision | Shown | Median s |
|---|---|---|---|---|---|
| Opus 5.5, ceiling (`claude -p`, effort high) | n/a | 1.00 | 0.49 | 4.1 | 14.6 |
| Qwen3.5-4B Q4_K_M | 256 | 0.74 | 0.42 | 2.5 | 15.6 |
| Qwen3.5-4B Q4_K_M | 4096 | 0.76 | 0.41 | 2.6 | 16.5 |
| Qwen3.5-4B Q4_K_M | off | 0.59 | 0.40 | 2.2 | 8.9 |
| gemma-4-E4B QAT Q4_0 | 256 | 0.65 | 0.37 | 2.3 | 8.5 |
| Nemotron-3-Nano-4B Q4_K_M | off | 0.54 | 0.25 | 3.0 | 2.2 (no tool calls) |

Below 4B nothing reached instant top-3; the sub-1B models found nothing. Qwen3.5-4B thinks 1.7-1.9k tokens per query at any budget from 256 up, and its levels differ by less than the noise of 78 runs. granite-4.2-3b and the 9B reference ran with the reranker unable to load beside them (the 3.9 GB build without flash-attn), so their numbers are not comparable.

The ceiling ran as fresh Claude Code sessions with only the four tools (bench/README.md, "Agent"); its transcripts are evaluation only, never training data (Training data, above). It found every query in every run, so the harness and tools suffice, and the 4B models' misses are the model's. Its precision understates it: the answer lists hold one or two places, and it also shows tests and exception classes that arguably count.

Speed on the 3080 (`bench/results/2026-10-06-222644-llm-speed.json`): no dense model above 4B generates 100 tok/s at 16k context; Qwen3.5-4B does about 140.
