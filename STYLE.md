# Writing rules for git-edge documents

These rules follow ASD-STE100 Simplified Technical English, with an "explain it to a beginner" goal on top. Every document in `findings/` and `plain/` must obey them.

## Sentence rules

1. Write one idea in one sentence.
2. Keep descriptive sentences to 25 words or fewer. Keep instructions to 20 words or fewer.
3. Use the active voice. Write "The server writes the file", not "The file is written by the server".
4. Use the present tense for facts. Use the simple past only for what happened in the study.
5. Start an instruction with a verb. Write "Write the object first."
6. Use "must" for a requirement. Do not use "should", "may", or "might" for a requirement.
7. Do not use more than three nouns in a row. Write "the cache for hot objects", not "hot object cache layer".
8. Do not use slang, idioms, or jokes.
9. Use the same word for the same thing every time. Do not vary words for style.
10. Keep a paragraph to six sentences or fewer.
11. Use a list when there are three or more parallel items.
12. Do not use "it" or "this" when the reader could ask "what?". Repeat the noun.
13. Do not use parentheses, em dashes, or semicolons. Start a new sentence instead.
14. Do not use the word "just", "simply", or "obviously".

## Word rules

Use these plain words. Define a technical word the first time it appears in a document, in one short sentence, using the glossary below.

| Do not write | Write |
|---|---|
| utilize | use |
| leverage | use |
| in order to | to |
| prior to | before |
| subsequent to | after |
| commence | start |
| terminate | stop, end |
| facilitate | help |
| mitigate | reduce |
| robust | strong, reliable |
| trivial | small, easy |
| non-trivial | large, hard |
| interoperate | work with |
| semantics | meaning, rules |
| primitive | building block |
| idempotent | safe to repeat |
| atomic | all at once, or not at all |
| race, race condition | two tasks collide |
| split-brain | two records disagree |
| orphan | lost file, file nobody points to |
| lookalike | a weaker version |
| hand-wave | skip, leave out |

## Structure for each explainer in `plain/`

Use these headings, in this order, with these exact words.

```
# <Idea title>

## What this idea is
## How it works
## What the reviewer decided
## Problems that must be fixed first   (only if there are blockers)
## Things to know                       (only if there are caveats)
## How this idea connects to the others
```

Rules for the sections:

- **What this idea is.** Two to four sentences a beginner can follow. Then one analogy from daily life, nature, or music. Mark the analogy with the words "Think of it like this."
- **How it works.** A numbered list of the steps, in order. Then one Mermaid diagram in a fenced block that starts with ` ```mermaid `. Use `sequenceDiagram` for request flows and `flowchart LR` for data layout. Keep the diagram to eight nodes or fewer.
- **What the reviewer decided.** State the verdict in one sentence. Then give the three scores as a small table. Then explain in plain words what the verdict means for this idea.
- **Problems that must be fixed first.** One subsection per blocker, with the heading `### Problem N: <short name>`. In each subsection, write three short paragraphs with the bold labels **What goes wrong.**, **Why it matters.**, and **How to fix it.**
- **Things to know.** One bullet per caveat. Each bullet is one or two plain sentences.
- **How this idea connects to the others.** One sentence per dependency, with a link to that idea's explainer: `[#N Title](./slug.md)`.

## Glossary

Use these definitions word for word the first time a term appears.

- **git**: a tool that keeps every version of a set of files, and lets many people share those versions.
- **repository, repo**: one project's full set of files and their history.
- **commit**: one saved version of the files, with a note about what changed.
- **branch**: a named line of commits, like a bookmark that moves forward as you save.
- **ref**: a name that points at one commit. A branch is a ref. A tag is a ref.
- **object**: one stored item in git. An object is a file's content, a folder listing, or a commit.
- **SHA, hash**: a fingerprint of an object's content. Two objects with the same content have the same fingerprint.
- **content-addressed**: stored under its own fingerprint, so the name tells you what is inside.
- **packfile, pack**: one bundle that holds many objects, squeezed to save space.
- **delta**: a stored object written as "the same as that other object, with these changes".
- **thin pack**: a packfile that contains deltas against objects the server already has.
- **push**: sending your new commits to the server.
- **fetch, clone**: getting commits from the server. A clone gets everything for the first time.
- **smart HTTP**: the way git talks to a server over normal web requests.
- **protocol v2**: the newer, cleaner set of messages git uses to talk to a server.
- **pkt-line**: git's way of framing a message. Each line starts with four characters that give its length.
- **sideband**: a way to send two kinds of data in one stream, like a main channel and a progress channel.
- **compare-and-swap, CAS**: change a value only if it still has the value you expect. If someone changed it first, do nothing and report it.
- **Cloudflare Workers, Worker**: small programs that run on Cloudflare's network close to the user, with no server to manage.
- **Durable Object, DO**: a single small program with its own storage that handles one thing at a time. There is one DO for each repo. Think of it like the one librarian who is allowed to update the catalog.
- **DO SQLite**: the small database inside each Durable Object.
- **alarm**: a timer inside a Durable Object. A DO has only one alarm at a time.
- **input gate**: the rule that a Durable Object handles one request at a time while it waits on its own storage. The gate opens when the DO waits on the network instead.
- **R2**: Cloudflare's large file store. It holds the git objects.
- **KV**: Cloudflare's small, fast, world-wide store for simple values.
- **subrequest**: one call from a Worker to another service, such as one read from R2. Each request may make at most 50 subrequests on the free plan and 10,000 on the paid plan.
- **Wasm, WebAssembly**: a way to run code from other languages, such as Rust, inside a Worker.
- **GA, generally available**: a Cloudflare feature that is finished and supported, not a preview.
- **janitor, sweep, GC**: a background task that deletes files nobody points to anymore.
- **blocker**: a problem that stops the idea from working until it is fixed.
- **caveat**: a limit or a condition. The idea works, but only inside this limit.

## The four verdicts

Use these exact explanations.

- **Lands.** The proof works on Cloudflare today. The reviewer found no problem that must be fixed first. Think of it like a flight with a clear runway.
- **Lands with caveats.** The idea is sound and can be built. The proof has one or more problems that must be fixed first, and the reviewer described each fix. Think of it like a flight with a runway that needs some repairs before you land. The runway is there. The repairs are known.
- **Risky.** The idea can be built. But the proof shows one or more problems the reviewer could not fully solve, or it does a weaker version of the goal. Think of it like a runway you can see on the map, but nobody has checked it for holes.
- **Does not land.** The idea does not work as stated. The reviewer showed the exact reason. There may be a different, weaker idea that does work. Think of it like a runway that turned out to be a lake.
