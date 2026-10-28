# Registers once (config persisted in /home/runner/actions-runner/state), then runs.
set -e
cd /home/runner/actions-runner
if [ ! -f state/.runner ]; then
  : "${RUNNER_TOKEN:?first start needs RUNNER_TOKEN}"
  ./config.sh --unattended --url https://github.com/Allternit/allternit-platform \
    --token "$RUNNER_TOKEN" --name allternit-standby-api --labels allternit-api-builder \
    --work /home/runner/work --replace
  mkdir -p state && cp .runner .credentials .credentials_rsaparams state/
else
  cp state/.runner state/.credentials state/.credentials_rsaparams .
fi
exec ./run.sh
