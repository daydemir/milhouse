# Milhouse

> autonomous ralph loops, but with glasses

<p align="center">
  <img src="assets/milhouse.webp" alt="Milhouse" height="300">
</p>

Milhouse hands a coding task to AI agents and keeps them working until an independent check says it's actually done.

A [Ralph Loop](https://ghuntley.com/ralph/) runs an agent on the same task over and over until it finishes. Milhouse keeps the loop and adds the glasses: a written contract that says what "done" means, and a separate reviewer that checks the work against it.

> [!NOTE]
> **This is a historical project.** It's published so you can read how it works and borrow ideas. It won't run as-is, because it depends on Agent Bridge and `rolly`, private tools that aren't included here. Its newer, prompt-only successor, [`climb`](https://github.com/daydemir/dstack/tree/main/skills/climb), lives in dstack.

> [!CAUTION]
> **Use at your own risk.** The builder runs Codex with its sandbox and approval prompts turned off, so it can change anything your user account can. Run it in a sandboxed environment.

## Goals

- **Top quality on every task.** Agents tend to take the light path. Milhouse defaults to thorough, and never lets an agent decide to skip that.
- **Affordable thoroughness.** Expensive models do the judgment calls. Cheaper flat-rate models do the heavy lifting: building, testing, and retrying.
- **Proof, not promises.** The agent doing the work doesn't get to say it's finished. A fresh reviewer decides, using the evidence.
- **A small, strict harness.** Code handles only what has to be exact: locks, run records, and exit results. Everything that takes judgment is left to the models.
- **Works on any project.** Milhouse isn't tied to a language, framework, or issue tracker. Project knowledge comes from your prompt and repo.

## How it works

1. **Plan.** `mil plan` talks the task through with you and writes `contract.md`, the definition of done.
2. **Freeze.** `mil build` checks the contract and locks it. If it changes mid-run, the run stops.
3. **Build.** A Codex agent does the work and records its evidence.
4. **Review.** A fresh, read-only agent checks the work against the contract.
5. **Loop.** If the review fails, the feedback goes back to the builder and it tries again. This repeats until the review passes, the run gets blocked, or it hits its limit.
6. **Grade.** Three blind judges can score the finished run with `mil grade`, each looking for something different: correctness, minimal changes, and honest evidence.

## Running it

You'll need Rust, [Claude Code](https://code.claude.com/docs/en/overview), [Codex](https://github.com/openai/codex), Agent Bridge at `~/.local/bin/agent-bridge`, and `rolly` for planning.

```bash
git clone https://github.com/daydemir/milhouse
cd milhouse
cargo run --release -- install     # installs mil and its agent skills

mil doctor                         # checks your setup
mil plan --workdir /path/to/repo "make the map load in under a second"
mil build --workdir /path/to/repo --contract contract.md --id map-speed
```

Keep the clone around, because the installed `mil` reads its prompts and skills from there. Run results are saved in `~/.milhouse`.

## Commands

| Command | What it does |
|---|---|
| `mil plan` | Works out the task with you and writes the contract |
| `mil contract validate` | Checks that a contract is ready to build |
| `mil contract render` | Makes a readable HTML version of a contract |
| `mil build` | Runs the build and review loop |
| `mil resume` | Continues an earlier run |
| `mil cancel` | Stops a run |
| `mil grade` | Scores a finished run with blind judges |
| `mil runs status` | Shows run storage and what can be cleaned up |
| `mil models` | Lists the available agents |
| `mil config` | Picks which agent handles each phase |
| `mil doctor` | Checks your setup |
| `mil dashboard` | Opens a local dashboard for runs and settings |
| `mil bench` | Runs your own benchmark tasks ([how](bench/README.md)) |

`mil build` exits with `0` when the run passes, `2` when it fails, and `3` when it's blocked.

For the architecture, read [`docs/spec.md`](docs/spec.md).

## Inspiration

- [Ralph](https://ghuntley.com/ralph/) by Geoffrey Huntley, the loop this all builds on
- [We need to talk about Ralph](https://youtu.be/Yr9O6KFwbW4) by Theo
- [Wayfinder](https://github.com/mattpocock/skills/tree/e9fcdf95b402d360f90f1db8d776d5dd450f9234/skills/engineering/wayfinder) by Matt Pocock, the basis for `mil plan` ([license](THIRD_PARTY_LICENSES.md))
