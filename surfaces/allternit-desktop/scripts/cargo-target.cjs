/**
 * Where cargo put its output: the shared target dir first (AGENTS.md — every
 * session exports CARGO_TARGET_DIR), then the repo's ./target. Scripts that
 * looked only in ./target failed with "binary not found" after a
 * shared-cache build.
 */
const path = require('path');

function cargoTargetDirs(repoRoot) {
  const shared = process.env.CARGO_TARGET_DIR ? path.resolve(repoRoot, process.env.CARGO_TARGET_DIR) : null;
  const local = path.join(repoRoot, 'target');
  return shared && shared !== local ? [shared, local] : [local];
}

/** Candidate paths for a cargo binary, release before debug unless releaseOnly. */
function cargoBinaryCandidates(repoRoot, name, { releaseOnly = false } = {}) {
  const profiles = releaseOnly ? ['release'] : ['release', 'debug'];
  return cargoTargetDirs(repoRoot).flatMap((dir) => profiles.map((p) => path.join(dir, p, name)));
}

module.exports = { cargoTargetDirs, cargoBinaryCandidates };
