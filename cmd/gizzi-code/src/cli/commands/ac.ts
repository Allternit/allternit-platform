/**
 * Agent Communication CLI (ac - Agent Communicate)
 * 
 * CLI tool for agents to communicate and coordinate, mounted as
 * `gizzi orchestration ac`. Its send/read (local JSONL) were folded into
 * the engine's `gizzi orchestration send` / `feed`; what remains here is
 * the local channel, key, rate-limit and git-bundle tooling.
 */

import { AgentAuth } from '@/runtime/integrations/agent-auth/agent-auth'
import { AgentRateLimiter } from '@/runtime/integrations/rate-limiter/rate-limiter'
import { GitBundle } from '@/runtime/integrations/git-bundle/git-bundle'
import { GitDAGTracker } from '@/runtime/integrations/git-dag/dag-tracker'
import { AgentWorkspaceCommunication } from '@/runtime/integrations/agent-workspace-communication'
import { cmd } from './cmd'

export const AcCommand = cmd({
  command: "ac",
  describe: "local agent channels, API keys, rate limits and git bundles",
  builder: (yargs) =>
    yargs
      .command(
        'channels',
        'List available channels',
        () => {},
        async () => {
          await AgentWorkspaceCommunication.initialize()

          const channels = await AgentWorkspaceCommunication.readChannels()

          console.log(`📺 Channels (${channels.length}):`)
          for (const ch of channels) {
            console.log(`   #${ch.name} (${ch.members.length} members)`)
          }
        },
      )
      .command(
        'join <channel>',
        'Join a channel',
        (yargs) =>
          yargs.positional('channel', {
            type: 'string',
            describe: 'Channel name',
          }),
        async (argv) => {
          await AgentWorkspaceCommunication.initialize()

          const channels = await AgentWorkspaceCommunication.readChannels()
          const channel = channels.find((c) => c.name === argv.channel)

          if (!channel) {
            console.error(`❌ Channel not found: ${argv.channel}`)
            process.exit(1)
          }

          await AgentWorkspaceCommunication.joinChannel(channel.id, 'cli-agent')
          console.log(`✅ Joined #${argv.channel}`)
        },
      )
      .command(
        'auth create',
        'Create a new API key for an agent',
        (yargs) =>
          yargs
            .option('agent-id', {
              type: 'string',
              required: true,
              describe: 'Agent ID',
            })
            .option('agent-name', {
              type: 'string',
              required: true,
              describe: 'Agent name',
            })
            .option('expires', {
              type: 'number',
              describe: 'Expiration in days',
            }),
        async (argv) => {
          const expiresAt = argv.expires
            ? Date.now() + (argv.expires as number) * 24 * 60 * 60 * 1000
            : undefined

          const result = AgentAuth.generateKey({
            agentId: argv.agentId as string,
            agentName: argv.agentName as string,
            agentRole: 'agent',
            expiresAt,
          })

          console.log(`✅ API key created for ${argv.agentName}`)
          console.log(`   Key ID: ${result.key.id}`)
          console.log(`   Key Prefix: ${result.key.keyPrefix}...`)
          console.log(`   ⚠️  Full key (save this now - won't be shown again):`)
          console.log(`   ${result.plainTextKey}`)
          console.log(`   Expires: ${expiresAt ? new Date(expiresAt).toISOString() : 'never'}`)
        },
      )
      .command(
        'auth list',
        'List API keys for an agent',
        (yargs) =>
          yargs.option('agent-id', {
            type: 'string',
            required: true,
            describe: 'Agent ID',
          }),
        async (argv) => {
          const keys = AgentAuth.listAgentKeys(argv.agentId as string)

          console.log(`🔑 API keys for ${argv.agentId} (${keys.length}):`)
          for (const key of keys) {
            const status = key.revoked
              ? '❌ Revoked'
              : key.expiresAt && key.expiresAt < Date.now()
                ? '❌ Expired'
                : '✅ Active'
            console.log(`   ${key.keyPrefix}... - ${status}`)
            console.log(`      Created: ${new Date(key.createdAt!).toISOString()}`)
            console.log(`      Usage: ${key.usageCount} requests`)
          }
        },
      )
      .command(
        'auth revoke <key-id>',
        'Revoke an API key',
        (yargs) =>
          yargs.positional('key-id', {
            type: 'string',
            describe: 'Key ID to revoke',
          }),
        async (argv) => {
          const success = AgentAuth.revokeKey(argv.keyId as string)

          if (success) {
            console.log(`✅ Key revoked: ${argv.keyId}`)
          } else {
            console.error(`❌ Key not found: ${argv.keyId}`)
            process.exit(1)
          }
        },
      )
      .command(
        'rate-limit show',
        'Show rate limit usage',
        (yargs) =>
          yargs.option('agent-id', {
            type: 'string',
            required: true,
            describe: 'Agent ID',
          }),
        async (argv) => {
          const usage = AgentRateLimiter.getAllUsage(argv.agentId as string)

          console.log(`📊 Rate limit usage for ${argv.agentId}:`)
          for (const [action, stats] of Object.entries(usage)) {
            const pct = ((stats.current / stats.limit) * 100).toFixed(1)
            const bar = '█'.repeat(Math.floor(Number(pct) / 10)) + '░'.repeat(10 - Math.floor(Number(pct) / 10))
            console.log(`   ${action}:`)
            console.log(`      [${bar}] ${stats.current}/${stats.limit} (${pct}%)`)
          }
        },
      )
      .command(
        'git leaves',
        'Get frontier commits (like agenthub ah leaves)',
        (yargs) =>
          yargs.option('repo', {
            type: 'string',
            required: true,
            describe: 'Repository path',
          }),
        async (argv) => {
          await GitDAGTracker.initialize(argv.repo as string)

          const frontier = GitDAGTracker.getFrontier()

          console.log(`🌿 Frontier commits (${frontier.length}):`)
          for (const hash of frontier.slice(0, 10)) {
            const commit = GitDAGTracker.getCommit(hash)
            if (commit) {
              console.log(`   ${commit.shortHash} - ${commit.message.slice(0, 50)}`)
            }
          }
          if (frontier.length > 10) {
            console.log(`   ... and ${frontier.length - 10} more`)
          }
        },
      )
      .command(
        'git lineage <commit-hash>',
        'Trace commit ancestry (like agenthub ah lineage)',
        (yargs) =>
          yargs.positional('commit-hash', {
            type: 'string',
            describe: 'Commit hash',
          }),
        async (argv) => {
          const lineage = GitDAGTracker.getLineage(argv.commitHash as string)

          console.log(`🌳 Lineage for ${argv.commitHash} (depth: ${lineage.depth}):`)
          for (const hash of lineage.path.slice(0, 10)) {
            const commit = GitDAGTracker.getCommit(hash)
            if (commit) {
              console.log(`   ${commit.shortHash} - ${commit.message.slice(0, 50)}`)
            }
          }
        },
      )
      .command(
        'git children <commit-hash>',
        'Find commits based on a commit (like agenthub ah children)',
        (yargs) =>
          yargs.positional('commit-hash', {
            type: 'string',
            describe: 'Commit hash',
          }),
        async (argv) => {
          const children = GitDAGTracker.getChildren(argv.commitHash as string)

          console.log(`👶 Children of ${argv.commitHash} (${children.length}):`)
          for (const hash of children.slice(0, 10)) {
            const commit = GitDAGTracker.getCommit(hash)
            if (commit) {
              console.log(`   ${commit.shortHash} - ${commit.message.slice(0, 50)}`)
            }
          }
        },
      )
      .command(
        'git bundle create [refs...]',
        'Create a git bundle',
        (yargs) =>
          yargs
            .positional('refs', {
              type: 'string',
              array: true,
              describe: 'Git refs to bundle',
            })
            .option('repo', {
              type: 'string',
              required: true,
              describe: 'Repository path',
            })
            .option('agent-id', {
              type: 'string',
              required: true,
              describe: 'Agent ID',
            })
            .option('output', {
              type: 'string',
              describe: 'Output path',
            }),
        async (argv) => {
          const bundle = await GitBundle.createBundle(
            argv.repo as string,
            (argv.refs as string[]) || ['HEAD'],
            argv.agentId as string,
            'CLI Agent',
            argv.output as string,
          )

          console.log(`✅ Bundle created: ${bundle.id}`)
          console.log(`   Path: ${bundle.path}`)
          console.log(`   Size: ${(bundle.size / 1024 / 1024).toFixed(2)} MB`)
          console.log(`   Commits: ${bundle.commits.length}`)
        },
      )
      .command(
        'git bundle validate <bundle-path>',
        'Validate a git bundle',
        (yargs) =>
          yargs.positional('bundle-path', {
            type: 'string',
            describe: 'Bundle file path',
          }),
        async (argv) => {
          const result = await GitBundle.validateBundle(argv.bundlePath as string)

          if (result.valid) {
            console.log(`✅ Bundle is valid`)
            console.log(`   Commits: ${result.commits?.length}`)
            console.log(`   Refs: ${result.refs?.length}`)
            console.log(`   Size: ${(result.size! / 1024 / 1024).toFixed(2)} MB`)
          } else {
            console.error(`❌ Bundle is invalid`)
            console.error(`   Error: ${result.error}`)
            process.exit(1)
          }
        },
      )
      .command(
        'git bundle extract <bundle-id> <repo-path>',
        'Extract a git bundle to a repository',
        (yargs) =>
          yargs
            .positional('bundle-id', {
              type: 'string',
              describe: 'Bundle ID',
            })
            .positional('repo-path', {
              type: 'string',
              describe: 'Target repository path',
            }),
        async (argv) => {
          const result = await GitBundle.extractBundle(
            argv.bundleId as string,
            argv.repoPath as string,
          )

          if (result.success) {
            console.log(`✅ Bundle extracted`)
            console.log(`   Commits added: ${result.commitsAdded}`)
          } else {
            console.error(`❌ Bundle extraction failed`)
            console.error(`   Error: ${result.error}`)
            process.exit(1)
          }
        },
      )
      .demandCommand(1),
  handler: async () => {},
})
