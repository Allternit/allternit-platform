import { Server } from "@/runtime/server/server"
import { Mesh } from "@/runtime/server/mesh"
import { cmd } from "@/cli/commands/cmd"
import { withNetworkOptions, resolveNetworkOptions } from "@/cli/network"
import { Flag } from "@/runtime/context/flag/flag"
import { Config } from "@/runtime/context/config/config"
import { init as initGlobal } from "@/runtime/context/global"
import { assertSafeServerExposure } from "@/cli/server-exposure"
import { ProcessRegistry } from "@/runtime/process-registry"
import { Sidecar } from "@/runtime/sidecar"

export const ServeCommand = cmd({
  command: "serve",
  builder: (yargs) =>
    withNetworkOptions(yargs)
      .option("allow-insecure-network", {
        type: "boolean",
        default: false,
        describe: "allow a non-loopback server without GIZZI_SERVER_PASSWORD (unsafe)",
      })
      .option("tunnel", {
        type: "boolean",
        default: false,
        describe: "expose the server through a cloudflared quick tunnel (requires cloudflared)",
      })
      .option("tunnel-token", {
        type: "string",
        describe:
          "run a Cloudflare named tunnel with this token (from `cloudflared tunnel token <name>`); implies --tunnel",
      })
      .option("tunnel-hostname", {
        type: "string",
        describe: "public hostname mapped to the named tunnel (enables the stable URL in logs and registration)",
      })
      .option("mesh", {
        type: "boolean",
        default: false,
        describe: "join the Allternit mesh tailnet (Tailscale/Headscale) so tailnet devices can reach this server",
      })
      .option("mesh-auth-key", {
        type: "string",
        describe: "Tailscale/Headscale preauth key for the mesh join; implies --mesh (secret — prefer GIZZI_MESH_AUTH_KEY)",
      })
      .option("mesh-control-url", {
        type: "string",
        describe: "Headscale coordination server URL (default https://mail.news.allternit.com:8444)",
      }),
  describe: "starts a headless gizzi server",
  handler: async (args) => {
    await initGlobal()
    const opts = await resolveNetworkOptions(args)
    const tunnelExplicitlySet = process.argv.includes("--tunnel") || process.argv.includes("--no-tunnel")
    const config = await Config.global()
    const tunnelFlag = tunnelExplicitlySet ? args.tunnel : (config?.server?.tunnel ?? args.tunnel)
    const tunnelToken = args.tunnelToken ?? Flag.GIZZI_TUNNEL_TOKEN ?? config?.server?.tunnel_token
    const tunnelHostname = args.tunnelHostname ?? Flag.GIZZI_TUNNEL_HOSTNAME ?? config?.server?.tunnel_hostname
    // A named-tunnel token implies tunnel mode even without --tunnel.
    const tunnel = tunnelFlag || !!tunnelToken

    const meshExplicitlySet = process.argv.includes("--mesh") || process.argv.includes("--no-mesh")
    const meshFlag = meshExplicitlySet ? args.mesh : (config?.server?.mesh ?? args.mesh)
    const meshAuthKey = args.meshAuthKey ?? Flag.GIZZI_MESH_AUTH_KEY ?? config?.server?.mesh_auth_key
    const meshControlUrl =
      args.meshControlUrl ?? Flag.GIZZI_MESH_CONTROL_URL ?? config?.server?.mesh_control_url ?? Mesh.DEFAULT_CONTROL_URL
    // An auth key implies mesh mode even without --mesh (same rule as --tunnel-token).
    const mesh = meshFlag || !!meshAuthKey

    // A tunnel exposes even a loopback server to the public internet, so the
    // same "never expose an unauthenticated server" rule applies. Shared with
    // `gizzi web` via server-exposure.ts so both commands enforce identical
    // auth policy.
    assertSafeServerExposure({
      command: "serve",
      hostname: opts.hostname,
      tunnel,
      allowInsecureNetwork: args.allowInsecureNetwork,
    })
    const server = Server.listen({ ...opts, tunnel, tunnelToken, tunnelHostname, mesh, meshAuthKey, meshControlUrl })
    process.stderr.write(`gizzi server listening on http://${server.hostname}:${server.port}\n`)
    // Graceful shutdown: server.stop() is the wrapped stop from Server.listen,
    // which also kills the cloudflared/mesh-node children. Without these
    // handlers SIGTERM/SIGINT would leave those children orphaned.
    let shuttingDown = false
    const shutdown = (signal: string) => {
      // A second signal while cleanup is in flight means "stop waiting".
      if (shuttingDown) {
        process.stderr.write(`gizzi server received ${signal} again; force exiting\n`)
        process.exit(1)
      }
      shuttingDown = true
      process.stderr.write(`gizzi server received ${signal}; shutting down\n`)
      ProcessRegistry.killAll()
      void Sidecar.stop().catch(() => {})
      server
        .stop()
        .catch(() => {
          // stop() rejects if the socket is already closed; exiting anyway.
        })
        .finally(() => process.exit(0))
    }
    ProcessRegistry.install()
    process.on("SIGINT", () => shutdown("SIGINT"))
    process.on("SIGTERM", () => shutdown("SIGTERM"))
    if (process.platform !== "win32") {
      process.on("SIGHUP", () => shutdown("SIGHUP"))
    }
    await new Promise(() => {})
  },
})
