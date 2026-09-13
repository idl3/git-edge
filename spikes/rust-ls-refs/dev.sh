#!/bin/bash
# Start wrangler dev for the spike on :8798 (logs to dev.log). Kill with: pkill -f "wrangler dev"
cd "$(dirname "$0")"
export PATH=/root/.cargo/bin:$PATH
exec npx wrangler dev --port 8798 --ip 127.0.0.1 "$@"
