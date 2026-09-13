# Platform facts spike

A tiny Worker plus one Durable Object that answers the platform questions the reviews left open. It runs on the real workerd runtime locally with `npx wrangler dev`, so the Durable Object, SQLite, alarm, and zlib answers are authoritative. R2 is simulated locally, so the multipart result shows the API shape only, and the subrequest limit is not enforced locally.

Run:

    npm install
    npx wrangler dev --port 8799
    curl localhost:8799/rows-written
    curl localhost:8799/zlib
    curl "localhost:8799/gate?n=8"
    curl localhost:8799/id-name
    curl localhost:8799/alarm ; sleep 3 ; curl localhost:8799/alarm-result
    curl localhost:8799/multipart

Results are recorded in `../../research/platform-facts.md`. To run against real R2 and real limits, deploy with `npx wrangler deploy` after creating an R2 bucket named `git-edge-spike`.
