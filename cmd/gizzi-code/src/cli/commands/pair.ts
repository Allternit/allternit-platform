import { cmd } from "@/cli/commands/cmd"
import { Pairing } from "@/runtime/services/pairing/pairing"

function printStatus(status: Awaited<ReturnType<typeof Pairing.status>>) {
  if (!status.paired && !status.stored) {
    process.stdout.write("Not paired. Run `gizzi pair` to pair this machine as a runtime device.\n")
    return
  }
  const stored = status.stored!
  process.stdout.write(`Device name:  ${stored.name}\n`)
  process.stdout.write(`Runtime ID:   ${stored.runtimeId ?? "(not paired)"}\n`)
  if (stored.userEmail) process.stdout.write(`Account:      ${stored.userEmail}\n`)
  process.stdout.write(`Fingerprint:  ${stored.publicKeyFingerprint}\n`)
  if (stored.tokenExpiresAt) {
    const remaining = Date.parse(stored.tokenExpiresAt) - Date.now()
    const days = Math.max(0, Math.floor(remaining / 86_400_000))
    process.stdout.write(`Token:        ${status.paired ? `valid (${days} days remaining, expires ${stored.tokenExpiresAt})` : `expired (${stored.tokenExpiresAt})`}\n`)
    if (Pairing.rotationDue(stored)) {
      process.stdout.write("Token rotation is due; it happens automatically the next time `gizzi serve` runs.\n")
    }
  }
  if (!status.paired) {
    process.stdout.write("Run `gizzi pair` to (re-)pair this machine.\n")
  }
}

async function runPair(args: { status?: boolean; force?: boolean; name?: string; login?: boolean }) {
  if (args.status) {
    printStatus(await Pairing.status())
    process.exit(0)
  }

  const existing = await Pairing.status()
  if (existing.paired && !args.force) {
    if (args.login) {
      process.stdout.write("Already signed in.\n")
    } else {
      process.stdout.write("This machine is already paired.\n")
    }
    printStatus(existing)
    process.stdout.write("Run `gizzi pair --force` to re-pair.\n")
    process.exit(0)
  }

  if (args.login) {
    process.stdout.write("Opening Allternit to sign in…\n")
  }

  const again = args.login ? "gizzi login" : "gizzi pair"
  const stored = await Pairing.pair({
    name: args.name,
    onCreated: (pairing) => {
      process.stdout.write(`Pairing code: ${pairing.userCode}\n`)
      if (Pairing.desktopApproval()) {
        process.stdout.write("Approve it in Allternit Desktop (opening it now), where you're already signed in.\n")
        process.stdout.write(`Or approve in a browser: ${pairing.verificationUrl}\n`)
      } else {
        process.stdout.write(`Opening ${pairing.verificationUrl} in your browser.\n`)
        process.stdout.write("Sign in with the account you use in Allternit Desktop, then click Connect runtime.\n")
      }
      process.stdout.write("Waiting for approval…\n")
    },
  }).catch((err) => {
    if (!(err instanceof Pairing.PairingEnded)) throw err
    process.stderr.write(
      err.reason === "expired"
        ? `The code expired before it was approved. Run \`${again}\` again.\n`
        : "Sign-in was cancelled.\n",
    )
    process.exit(1)
  })
  if (args.login) {
    process.stdout.write(`Signed in as ${stored.userEmail ?? stored.name}.\n`)
  } else {
    process.stdout.write(`Paired as ${stored.name} (runtime ${stored.runtimeId}).\n`)
  }
  if (stored.userEmail) process.stdout.write(`Account: ${stored.userEmail}\n`)
  process.stdout.write(`Device token valid until ${stored.tokenExpiresAt}.\n`)
  process.exit(0)
}

export const PairCommand = cmd({
  command: "pair",
  builder: (yargs) =>
    yargs
      .option("status", {
        type: "boolean",
        default: false,
        describe: "show the current pairing state and exit",
      })
      .option("force", {
        type: "boolean",
        default: false,
        describe: "re-pair even if a valid device token already exists",
      })
      .option("name", {
        type: "string",
        describe: "device name shown on the platform (defaults to the stored name or hostname)",
      }),
  describe: "pair this machine as an Allternit runtime device",
  handler: (args) => runPair({ ...args, login: false }),
})

export const LoginCommand = cmd({
  command: "login",
  builder: (yargs) =>
    yargs
      .option("force", {
        type: "boolean",
        default: false,
        describe: "re-sign in even if a valid device token already exists",
      })
      .option("name", {
        type: "string",
        describe: "device name shown on the platform (defaults to the stored name or hostname)",
      }),
  describe: "sign in to Allternit from the command line",
  handler: (args) => runPair({ ...args, login: true }),
})
