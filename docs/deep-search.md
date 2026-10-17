# Deep search

A small local agent that searches on the user's behalf, beside the instant search. Started 2026-10-06. `semsearch ask` (`src/ask.rs`, 2026-10-16) runs the loop of `bench/agent.py` for code. It reads its model from `SEMSEARCH_LLM_URL`, `SEMSEARCH_LLM_MODEL` and `SEMSEARCH_LLM_KEY`; the model is not in InferMux yet, and no UI calls it.

## What the user decided (2026-10-06)

- The search widget sits in the center of the screen, slightly above the middle.
- The instant results (embed, rerank, one item per file) appear below the input, as they do now. They stay fast and predictable.
- The deep search is one row above the input. Pressing Up from the input selects it, and selecting it starts the deep search for the current text.
- The agent chooses which results to show and shows only the best ones. It may run more searches to refine them and read the files the results point to, to check that they answer the question.
- It is a small agent with tools, not a one-pass explainer. Each result it keeps gets a line on why it answers the question or why it is relevant. Simple follow-up questions were mentioned as a possibility.
- Git history is in scope later: commits, branches, worktrees and other history.

## Why

The instant list is what a user expects from a search box: the same text gives the same list, at once. The agent costs seconds and can be wrong, so it is something the user asks for, not something that replaces the list.

## What the user decided (2026-10-16)

- **The loop runs in semsearch**, as `semsearch ask`, in the library so the desktop window can call it without a process. The CLI and agents get it too.
- **It prints progress and hits.** Text by default; `--json` gives JSON lines: a `step` event per tool call, a `hit` per kept place (path, lines, url, why), then `done`. The UI row can show progress during the seconds a deep search takes.
- **Code only in the first version.** The user first chose all sources, then code only once the live instance turned out to hold only code: on 2026-10-16 `type:web` and `type:file` each returned 0 of its 83,092 documents, so web and file tools would ship untested. Files and web get their own design later.
- **12 model turns and 60 s.** At 60 s the model is told to answer with what it has checked; the last turn has no tools either way.

How the 60 s holds is the agent's call (2026-10-17). A model call is cut off at what is left of the 60 s, and a tool call left in a turn once the time is up is answered "out of time" without running. Each tool, search included, gives up at 30 s. The answer turn then runs without thinking, for at most 30 s; the alternative was to let it think within the budget of 256. So a deep search ends by about 120 s at worst: 60 s, one tool's 30 s, and the answer's 30 s. Before, the limit was checked only between turns, and the live run below took 133.6 s: on the CPU each turn took about a minute, so turn 2 began under 60 s and ran past it, and the forced turn 3 thought too.
- **The agent's model and the reranker share the card**, so the agent's searches keep their rerank, as in the 2026-10-07 eval, without one model evicting the other. Chosen over a per-query switch that skips the rerank (the earlier recommendation) and over semsearch running its own llama-server beside InferMux, which the warden would not see. How InferMux keeps both loaded is open.

The model for the first version is Qwen3.5-4B Q4_K_M with a thinking budget of 256, the eval's best local result. That is the agent's pick, not a decision.

## Open

- **UI.** The row above the input goes in the desktop window and the VS Code modal of `docs/decisions.md`, 2026-10-10. Not built.
- **How the card is shared.** strix's GPU runs one model at a time (InferMux 0022, item 7). Keeping the reranker and the agent's model loaded together needs a change in InferMux.
- **Fine-tuning.** Not decided; where its training data may come from is under "Training data and terms of use" below.
- **Files and web.** Not indexed in the live instance, and `bench/` has queries for code only.
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

## Why-lines, graded 2026-10-17

Opus 5.5 (effort medium, `claude -p`, no tools) graded every why-line of two 2026-10-07 runs with `bench/why.py`, one call per query: is the line true of the code it points at (shown with 3 lines around it), and does it say why the place answers the question. Evaluation only, never training data. Results in `bench/results/2026-10-17-034403-why.json`.

| Run | Lines | True | Partly true | False | Says why | True and says why |
|---|---|---|---|---|---|---|
| Qwen3.5-4B Q4_K_M, budget 256 | 196 | 0.81 | 0.18 | 0.01 | 0.60 | 0.48 |
| of those, at a correct place | 82 | 0.88 | 0.12 | 0 | 0.80 | 0.70 |
| Opus 5.5, ceiling | 319 | 0.92 | 0.08 | 0 | 0.80 | 0.75 |
| of those, at a correct place | 155 | 0.93 | 0.07 | 0 | 0.88 | 0.82 |

Qwen almost never writes a false line; it writes a line that only restates the code ("Exception class representing a full shift") at four in ten. At the places that are correct, its lines are nearly as good as Opus's. Two caveats. Opus graded its own lines, so the ceiling may read high. And "partly" includes claims about code just outside the shown lines, such as the name of the enclosing function, so "true" is a lower bound for both.

## How `semsearch ask` differs from the eval

The agent's calls, 2026-10-16:

- Paths are relative to the home directory, since the indexed repositories sit under `~/Projects` and `~/Desktop/scratch`; the eval had one corpus root. The prompt lists the repositories as such paths, and `git` takes one, or a bare repository name.
- The tools see only files inside the repositories in `code-state.json`. grep looks in the `--in` directories, or every repository, rather than the whole root.
- git takes an allowlist of options per subcommand, parsed as git parses them: grouped short flags one by one, a short option's value attached or next, long options by exact name only, since git expands any unique prefix. Anything else is refused, which covers `--output`, `-O`/`--open-files-in-pager`, `--ext-diff`, `--textconv`, `--no-index`, `grep -f` and `blame --contents`. A path argument may not be absolute, start with `~` or climb out of the repository with `..`, because `git diff` with a path outside the work tree reads it as `--no-index` would. git runs with `--no-pager -c core.pager=cat`, `GIT_PAGER=cat` and no `GIT_EXTERNAL_DIFF`. Changed 2026-10-17, after a review ran a pager command through `grep -iO'cmd'` and `grep --open='cmd'`, and read a file outside the repositories through `diff --no-index`: a list of refused prefixes cannot follow git's parsing.
- A kept place whose file does not exist in the indexed code is left out, with a note, instead of shown.
- A bare `path:N` is the line N; the eval's lenient score mapped it to the piece starting there.

## Verification

- 2026-10-16: `cargo test` covers the loop with a scripted model: a read then a hit with its why and URL, the forced answer without tools at the time and turn limits, reads and grep outside the indexed repositories refused, hits there left out, and git's refusals. Removing the repository check, the time check or the `--output` refusal each failed one test.
- 2026-10-17: the git test runs each spelling the review used (`grep -iO'touch marker'`, `grep -inO 'touch marker'`, `grep --open='touch marker'`, `diff --no-index`, `diff ../secret.txt`, an absolute path, `-- src/../../secret.txt`) and abbreviations of allowed options (`--cont=3`, `--line`), and that grouped flags, values and counts still pass. Skipping the allowlist, the absolute-path check, exact long-name matching (valued and not) or the refusal of an unknown grouped flag each failed it.
- 2026-10-17: the time limits have their own tests: a turn gets only the time left after a slow search, an answer turn that runs out ends the run, a search hister accepts but never answers is given up, and a tool that does not finish is killed. A last test pins the documented 30 s for tools, searches and the answer turn, which a behaviour test could not check without waiting 30 s. Giving each turn the full budget, dropping the search timeout, dropping the break after a timed-out answer turn, letting `run_tool` ignore its timeout, or setting the tool timeout to 100,000 s each failed a test; before, all of them passed every test.
- 2026-10-16, live: `semsearch ask --json --in ~/Projects/hister "where are failed embedding jobs retried"` against Qwen3.5-4B Q4_K_M on a llama-server on the CPU (the card held Warframe), reasoning budget 256. The embedder answered 503 and the reranker `gpu_no_room`, so searches fell back to keyword hits, each reported as a note. Two tool calls, then the time limit at 133.6 s; one hit, `embedding_queue.go:274-285`, the `retry` function, a correct place. It missed the hourly requeue at line 142. Not yet run through InferMux or on the GPU, and the eval has not been rerun on this loop.
