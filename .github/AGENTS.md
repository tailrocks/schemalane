# GitHub workflow guidance

- Keep one workflow shape for Velnor, GitHub, and `both` lanes.
- Velnor is the default; GitHub uses pinned `ubuntu-26.04`.
- Pin every third-party action to a full commit SHA.
- Keep permissions least-privilege, concurrency bounded, and every job timed out.
- Preserve identical job and step semantics across lanes.
- Keep release publishing operator-controlled and single-writer gated.
- Keep the crates.io token scoped to the publish step.
- Publish crates in dependency order and skip versions already published.
- The PostgreSQL integration suite uses testcontainers and requires Docker.
- Never expose secrets to pull-request jobs or untrusted scripts.
