# Self-hosted CI runner (allternit-api builds)

Since 2026-10-27 the compile job of `.github/workflows/deploy-allternit-api-contabo.yml` runs on our own builder instead of a fresh GitHub runner. On a GitHub runner the compile alone was ~49 of the ~55 min. The builder keeps its Cargo target dir between runs, so only the crates a merge changed recompile.

| | |
|---|---|
| Host | allternit-standby (31.220.95.165, tailnet 100.83.199.24) |
| Container | `gh-runner-api`, image `allternit-gh-runner:<runner version>`, built from `infra/gh-runner/` |
| Runner name / label | `allternit-standby-api` / `allternit-api-builder` |
| Limits | 5 CPUs, 14 GB memory (the host also runs prod voice and the Linux runtime build) |
| State | `/srv/gh-runner/state` (registration), `/srv/gh-runner/work` (checkouts and `_cargo-target`) |

## Security

The repo is public, so these rules hold:

- The runner holds no secrets. The deploy job (SSH key, Tailscale) runs on a GitHub-hosted runner and gets the binary as an artifact.
- Only `push` to main and `workflow_dispatch` use the runner. Never add `pull_request` triggers that target `allternit-api-builder`.
- Fork PRs from anyone outside the org need approval before any workflow runs (repo setting `approval_policy=all_external_contributors`). A fork PR can add its own workflow that asks for this label, so check a fork PR's workflow changes before approving its run. Keep the setting.
- The container runs as an unprivileged user with `no-new-privileges` and no Docker socket.

## Operate

- Builder offline or broken: run the workflow by hand with `hosted: true` to build on a GitHub runner.
- Status: `gh api repos/Allternit/allternit-platform/actions/runners`; logs: `ssh root@31.220.95.165 docker logs gh-runner-api`.
- Disk: the target dir grows. If `/` on the builder is under ~20 GB free, delete `/srv/gh-runner/work/_cargo-target` (the next build is cold).
- Rebuild or re-register:

  ```bash
  scp infra/gh-runner/* root@31.220.95.165:/srv/gh-runner/
  ssh root@31.220.95.165 'cd /srv/gh-runner && docker build -t allternit-gh-runner:2.338.2 . \
    && docker rm -f gh-runner-api && rm -rf state/*'
  T=$(gh api -X POST repos/Allternit/allternit-platform/actions/runners/registration-token -q .token)
  ssh root@31.220.95.165 "docker run -d --name gh-runner-api --restart unless-stopped --cpus 5 --memory 14g \
    --memory-swap 18g --pids-limit 4096 --security-opt no-new-privileges -e RUNNER_TOKEN=$T \
    -v /srv/gh-runner/state:/home/runner/actions-runner/state -v /srv/gh-runner/work:/home/runner/work \
    allternit-gh-runner:2.338.2"
  ```

  The runner updates itself; rebuild the image only to change tools or the Rust toolchain (`rust-toolchain.toml`).
