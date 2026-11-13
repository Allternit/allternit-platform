/**
 * Side-effect module: installs the self-hosted egress guard as early as an
 * import can run. src/cli/main.ts imports this first so no other module's
 * top-level code can reach an Allternit host before the guard is in place.
 */
import { installSelfHostedGuard } from "./offline"

installSelfHostedGuard()
