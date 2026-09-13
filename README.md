# git-edge

git-edge is a plan for a git server that has no server. git is a tool that keeps every version of a set of files, and lets many people share those versions. Normally you need a computer that runs all day to share those versions. git-edge uses Cloudflare's network instead. Small programs run near the user. One small program per project keeps the list of branches. A large file store keeps the file contents. There is no machine to manage, patch, or restart.

Think of it like this. A normal git server is a shop with a shopkeeper who must be there all day, even when nobody comes. git-edge is a vending machine. It wakes up when someone presses a button, does one thing, and goes back to sleep. Many vending machines can stand in many cities, and each one knows exactly what is inside it.

## What is in this repository

This repository does not contain the server. It contains the study that comes before the server. We took 56 ideas. For each idea, one agent wrote a small proof in code to show how it could work on Cloudflare. Then a second agent tried to break that proof. The second agent looked for three things:

- Will it run on Cloudflare today?
- Can a crash or two users at once lose data?
- Does it do what the idea says, or something weaker?

The proofs have not run on a real Cloudflare account. Read them as an argument, and the reviews as the argument against. A small test program has run on the real runtime locally, and its answers are in [research/platform-facts.md](research/platform-facts.md).

## The result in one table

| Verdict | Count | What it means |
|---|---|---|
| Lands | 1 | Works today. No problem to fix first. |
| Lands with caveats | 33 | Sound idea. Known problems to fix first, each with a known fix. |
| Risky | 21 | Can be built. Some problems have no full fix yet, or the proof does something weaker than the idea. |
| Does not land | 1 | Does not work as stated. A weaker idea does work. |

The reviewers were told to doubt everything. So "lands with caveats" is the real pass mark. The one clean pass is the smallest piece: the first message a git program sends and the reply it expects. The one failure is a branch that many people edit at once, because a normal git program refuses that push before it leaves the user's machine.

**The main idea holds.** One small program per project, which changes a branch only if the branch still has the value it expects, does exactly what a normal git server does. Files stored under their own fingerprint are safe to write twice. No reviewer found a way for two users to end up with two different truths.

## Where to start reading

Read in this order.

1. [findings/how-to-read.md](findings/how-to-read.md). What the verdicts and the scores mean, in plain words.
2. [findings/cross-cutting-defects.md](findings/cross-cutting-defects.md). The nine problems that appeared again and again, with pictures, and the one fix for each.
3. [findings/build-order.md](findings/build-order.md). Six waves, from the first message to the exotic ideas.
4. [findings/scoreboard.md](findings/scoreboard.md). All 56 ideas scored, with a one-line plain summary each.
5. [findings/rust-and-gitoxide.md](findings/rust-and-gitoxide.md). Why the server will be written in Rust with gitoxide, and what that does and does not fix.
6. [findings/second-pass.md](findings/second-pass.md). What changed when 33 ideas were rewritten in Rust against one shared contract, in plain words.
7. [plain/](plain/). One plain-language explainer per idea, with an analogy, a diagram, and each problem explained.
8. [site/index.html](site/index.html). The interactive map. Click an idea to see its plain explainer, its proofs, and its reviews.

## The language decision

The 56 proofs were written in TypeScript. We then asked whether the server can be written in Rust. The answer is yes, with gitoxide, a set of Rust building blocks that already read and write git's file formats. gitoxide takes over the two hardest problems in the list below, the message format and the squeezed packfiles. The first library we looked at, git2-rs, does not run inside a Worker, and the reasons are in the finding. Read [findings/rust-and-gitoxide.md](findings/rust-and-gitoxide.md) for the plain version and [research/rust-server.md](research/rust-server.md) for the engineer memo with every source link.

## Map of the folders

| Folder | What is in it | Who it is for |
|---|---|---|
| `findings/` | The summary documents listed above | Everyone. Start here. |
| `plain/` | One explainer per idea in plain language | Everyone. |
| `proofs/` | One proof per idea, with code | Engineers. |
| `reviews/` | One review per idea, in the reviewer's own words | Engineers. |
| `proofs-v2/` | The second pass: 33 foundation and edge proofs rewritten in Rust against the contract | Engineers. |
| `reviews-v2/` | The second-pass reviews, checking contract compliance | Engineers. |
| `ideas/` | The list of 56 ideas as they went into the study | Everyone. |
| `site/` | The interactive map as one web page | Everyone. |
| `data/` | The structured results every other file was made from | Tools. |
| `research/` | Engineer-level memos with source links: the Rust check, and the platform facts we measured | Engineers. |
| `spikes/` | Small real programs run on the Cloudflare runtime to test a fact | Engineers. |
| `CONTRACTS.md` | The one set of rules every revised proof follows: storage layout, the ref transaction, the job timer, the janitor | Engineers. |
| `STYLE.md` | The writing rules and glossary for the plain documents | Writers. |

## The nine problems in one breath

These are the causes that kept coming back. The full document explains each with a picture.

1. git checks its message format byte by byte, and many proofs got a byte wrong.
2. The cleanup task can delete a file that a new push started to use.
3. Real pushes arrive squeezed, and many proofs expected them unsqueezed.
4. The proofs did not agree on how files are stored.
5. The small program lets requests collide while it waits on the network.
6. The small program has one timer, and everyone reset it.
7. git sends request bodies in ways the proofs did not expect.
8. One file read per object hits the subrequest limit.
9. The small program does not know its own name.

## How this study was made

Each idea went to one agent with a fixed brief. Show the mechanism in 40 to 120 lines of code on real Cloudflare building blocks. Name what you used. Name what you left out. Each proof then went to a second agent with a different brief. Doubt everything. Walk through one crash. Walk through two users at once. Name the exact byte that would break a real git program. The summary documents were made from those 112 outputs. The plain explainers were then written from the proofs and reviews, using the rules in `STYLE.md`.
