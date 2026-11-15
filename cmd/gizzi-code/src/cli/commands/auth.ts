import type { Argv } from "yargs"
import * as prompts from "@clack/prompts"
import path from "path"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { printComputeStatusAfterLogin } from "@/cli/commands/computers"
import { Global } from "@/runtime/context/global"
import {
  addAuthProfile,
  diagnoseAuth,
  getAuthStatus,
  loginApiKey,
  logout,
  readAuthProfiles,
  removeAuthProfile,
  setActiveAuthProfile,
  storeApiKeyForProfile,
} from "@/runtime/context/config/auth-profiles"

const configPath = () => path.join(Global.Path.config, "config.toml")

const LoginCommand = cmd({
  command: "login",
  describe: "sign in with an API key",
  builder: (yargs) =>
    yargs
      .option("api-key", { type: "string", describe: "API key" })
      .option("provider", { type: "string", describe: "provider id", default: "allternit" })
      .option("profile", { type: "string", describe: "profile name", default: "default" }),
  async handler(args) {
    let apiKey = args.apiKey
    if (!apiKey) {
      const answer = await prompts.password({
        message: "API key",
        validate: (value) => value?.trim() ? undefined : "Required",
      })
      if (prompts.isCancel(answer)) throw new UI.CancelledError()
      apiKey = answer.trim()
    }
    const result = await loginApiKey(configPath(), apiKey, {
      provider: args.provider,
      profile: args.profile,
    })
    UI.println(`Signed in with API key (${result.method}): ${result.profile}`)
    await printComputeStatusAfterLogin()
  },
})

const StatusCommand = cmd({
  command: "status",
  describe: "show active authentication method",
  async handler() {
    const status = await getAuthStatus(configPath())
    switch (status.method) {
      case "oauth_token":
        UI.println("Authenticated via OAuth token")
        break
      case "api_key":
        UI.println(`Authenticated via API key${status.profile ? `: ${status.profile}` : ""}`)
        break
      case "none":
      default:
        UI.println("Not authenticated")
        break
    }
  },
})

const LogoutCommand = cmd({
  command: "logout",
  describe: "sign out and remove stored credentials",
  builder: (yargs) =>
    yargs.option("profile", { type: "string", describe: "specific profile to remove" }),
  async handler(args) {
    const result = await logout(configPath(), args.profile)
    if (result.method === "oauth_token") {
      UI.println("Signed out of OAuth session")
    } else if (result.profile) {
      UI.println(`Removed auth profile: ${result.profile}`)
    } else {
      UI.println("Not authenticated")
    }
  },
})

const DiagnoseCommand = cmd({
  command: "diagnose",
  describe: "print authentication diagnostic information",
  async handler() {
    const diagnosis = await diagnoseAuth(configPath())
    UI.println(`Config path: ${diagnosis.config_path}`)
    UI.println(`Config exists: ${diagnosis.config_exists}`)
    UI.println(`Active method: ${diagnosis.method}`)
    UI.println(`Active profile: ${diagnosis.active_profile ?? "(none)"}`)
    UI.println(`Credential store: ${diagnosis.credential_store ?? "auto (default)"}`)
    UI.println(`Profiles (${diagnosis.profile_count}): ${diagnosis.profile_names.join(", ") || "(none)"}`)
    UI.println(`Runtime auth keys: ${diagnosis.runtime_auth_keys.join(", ") || "(none)"}`)
    UI.println("Environment variables:")
    for (const [name, present] of Object.entries(diagnosis.env_vars)) {
      UI.println(`  ${name}: ${present ? "set" : "not set"}`)
    }
  },
})

const ProfileListCommand = cmd({
  command: "list",
  describe: "list authentication profiles",
  async handler() {
    const auth = await readAuthProfiles(configPath())
    const names = Object.keys(auth.profiles).sort()
    if (names.length === 0) {
      UI.println("No authentication profiles configured.")
      return
    }
    for (const name of names) {
      const profile = auth.profiles[name]!
      UI.println(`${auth.active_profile === name ? "*" : " "} ${name} (${profile.provider})`)
    }
  },
})

const ProfileAddCommand = cmd({
  command: "add <name>",
  describe: "add an authentication profile",
  builder: (yargs) => yargs
    .positional("name", { type: "string", demandOption: true })
    .option("provider", { type: "string", describe: "provider id" })
    .option("api-key", { type: "string", describe: "API key" })
    .option("api-key-env", { type: "string", describe: "environment variable containing the API key" })
    .option("base-url", { type: "string", describe: "provider base URL" }),
  async handler(args) {
    const interactive = !args.provider && !args.apiKey && !args.apiKeyEnv && !args.baseUrl
    let provider = args.provider
    if (!provider) {
      const answer = await prompts.text({
        message: "Provider",
        validate: (value) => value?.trim() ? undefined : "Required",
      })
      if (prompts.isCancel(answer)) throw new UI.CancelledError()
      provider = answer.trim()
    }
    let apiKeyEnv = args.apiKeyEnv
    let baseUrl = args.baseUrl
    if (interactive) {
      const envAnswer = await prompts.text({
        message: "API key environment variable (optional)",
        placeholder: "ALLTERNIT_API_KEY",
      })
      if (prompts.isCancel(envAnswer)) throw new UI.CancelledError()
      apiKeyEnv = envAnswer.trim() || undefined
      const urlAnswer = await prompts.text({
        message: "Provider base URL (optional)",
      })
      if (prompts.isCancel(urlAnswer)) throw new UI.CancelledError()
      baseUrl = urlAnswer.trim() || undefined
    }
    await addAuthProfile(configPath(), args.name, {
      provider,
      api_key_env: apiKeyEnv,
      base_url: baseUrl,
    })
    if (args.apiKey) {
      // Keys go to the credential store — never inline into config.toml.
      await storeApiKeyForProfile(configPath(), args.name, args.apiKey)
    }
    UI.println(`Added auth profile: ${args.name}`)
  },
})

const ProfileRemoveCommand = cmd({
  command: "remove <name>",
  aliases: ["rm"],
  describe: "remove an authentication profile",
  builder: (yargs) => yargs.positional("name", { type: "string", demandOption: true }),
  async handler(args) {
    await removeAuthProfile(configPath(), args.name)
    UI.println(`Removed auth profile: ${args.name}`)
  },
})

const ProfileSetActiveCommand = cmd({
  command: "set-active <name>",
  describe: "select the active authentication profile",
  builder: (yargs) => yargs.positional("name", { type: "string", demandOption: true }),
  async handler(args) {
    await setActiveAuthProfile(configPath(), args.name)
    UI.println(`Active auth profile: ${args.name}`)
  },
})

const ProfileCommand = cmd({
  command: "profile",
  describe: "manage authentication profiles",
  builder: (yargs: Argv) => yargs
    .command(ProfileListCommand)
    .command(ProfileAddCommand)
    .command(ProfileRemoveCommand)
    .command(ProfileSetActiveCommand)
    .demandCommand(),
  async handler() {},
})

export const AuthCommand = cmd({
  command: "auth",
  describe: "manage authentication",
  builder: (yargs: Argv) =>
    yargs
      .command(LoginCommand)
      .command(StatusCommand)
      .command(LogoutCommand)
      .command(DiagnoseCommand)
      .command(ProfileCommand)
      .demandCommand(),
  async handler() {},
})
