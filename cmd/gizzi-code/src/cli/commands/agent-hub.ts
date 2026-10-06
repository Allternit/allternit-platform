/**
 * Agent Hub CLI Command
 *
 * Terminal interface for browsing and creating agents from specialist templates.
 * Supports interactive TUI mode and direct template selection.
 *
 * Usage:
 *   gizzi agents templates                    # Interactive TUI
 *   gizzi agents templates list               # List all templates
 *   gizzi agents templates create <template>  # Create from template
 *   gizzi agents templates export <agent-id>  # Export agent
 *   gizzi agents templates import <file>      # Import agent
 *
 * @module agent-hub-command
 */

import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap"
import * as prompts from "@clack/prompts"
import { SPECIALIST_TEMPLATES, getTemplateById, createAgentFromTemplate } from "@/lib/agents/agent-templates.specialist"
import type { SpecialistTemplate, AgentCategory } from "@/lib/agents/agent-templates.specialist"
import { AgentManager } from "@/runtime/loop/manager"
import { writeFileSync, readFileSync, existsSync } from "fs"
import path from "path"

// ============================================================================
// Main Command
// ============================================================================

export const AgentHubCommand = cmd({
  command: "templates",
  describe: "agent templates: browse, create, export, import",
  builder: (yargs: Argv) => {
    return yargs
      .command(AgentHubListCommand)
      .command(AgentHubCreateCommand)
      .command(AgentHubExportCommand)
      .command(AgentHubImportCommand)
      .command(AgentHubInteractiveCommand)
      .demandCommand(1, "Choose a command")
  },
  async handler() {
    // Default to interactive mode
    await AgentHubInteractiveCommand.handler!({} as any)
  },
})

// ============================================================================
// List Command
// ============================================================================

export const AgentHubListCommand = cmd({
  command: "list [category]",
  describe: "list available agent templates",
  builder: (yargs: Argv) => {
    return yargs
      .positional("category", {
        describe: "filter by category (engineering, design, marketing, etc.)",
        type: "string",
        choices: ["engineering", "design", "marketing", "product", "testing", "support", "specialized"],
      })
      .option("json", {
        alias: "j",
        describe: "output as JSON",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const category = args.category as AgentCategory | undefined
      const templates = category
        ? SPECIALIST_TEMPLATES.filter(t => t.category === category)
        : SPECIALIST_TEMPLATES

      if (args.json) {
        process.stdout.write(JSON.stringify(templates, null, 2) + "\n")
        return
      }

      // Group by category
      const byCategory = new Map<AgentCategory, typeof templates>()
      templates.forEach(t => {
        const existing = byCategory.get(t.category) || []
        existing.push(t)
        byCategory.set(t.category, existing)
      })

      prompts.intro("Agent Hub - Available Templates")
      
      for (const [category, categoryTemplates] of byCategory.entries()) {
        prompts.log.info(`${category.toUpperCase()} (${categoryTemplates.length})`)
        
        for (const template of categoryTemplates) {
          process.stdout.write(`  • ${template.name} - ${template.description}\n`)
        }
      }

      prompts.outro("Use 'gizzi agents templates create <template-id>' to create an agent")
    })
  },
})

// ============================================================================
// Create Command
// ============================================================================

export const AgentHubCreateCommand = cmd({
  command: "create <template>",
  describe: "create an agent from a template",
  builder: (yargs: Argv) => {
    return yargs
      .positional("template", {
        describe: "template ID or name",
        type: "string",
        demandOption: true,
      })
      .option("name", {
        alias: "n",
        describe: "agent name (defaults to template name)",
        type: "string",
      })
      .option("output", {
        alias: "o",
        describe: "output directory for agent workspace",
        type: "string",
      })
      .option("json", {
        alias: "j",
        describe: "output config as JSON",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      // Find template by ID or name
      const templateId = args.template as string
      const template = getTemplateById(templateId) || 
                       SPECIALIST_TEMPLATES.find(t => t.name.toLowerCase().includes(templateId.toLowerCase()))
      
      if (!template) {
        prompts.log.error(`Template not found: ${templateId}`)
        prompts.outro("Use 'gizzi agents templates list' to see available templates")
        process.exit(1)
      }

      // Show template preview
      prompts.intro(`Creating agent from: ${template.name}`)
      
      process.stdout.write(`\n${prompts.isCancel} Template: ${template.name}\n`)
      process.stdout.write(`${prompts.isCancel} Category: ${template.category}\n`)
      process.stdout.write(`${prompts.isCancel} Role: ${template.role}\n`)
      process.stdout.write(`${prompts.isCancel} Model: ${template.agentConfig.model}\n`)
      process.stdout.write(`${prompts.isCancel} Tools: ${template.agentConfig.tools?.join(", ") || "default"}\n`)
      process.stdout.write(`${prompts.isCancel} Capabilities: ${template.agentConfig.capabilities?.join(", ") || "default"}\n`)

      const agentName = (args.name as string) || template.name

      if (args.json) {
        const config = createAgentFromTemplate(template.id)
        process.stdout.write(JSON.stringify(config, null, 2) + "\n")
        return
      }

      // Interactive confirmation
      const confirm = await prompts.confirm({
        message: `Create agent "${agentName}" from this template?`,
      })

      if (prompts.isCancel(confirm) || !confirm) {
        prompts.cancel("Agent creation cancelled")
        return
      }

      const config = createAgentFromTemplate(template.id)

      try {
        const agent = await AgentManager.create({
          name: agentName as string,
          description: config.description,
          prompt: config.instructions || undefined,
          model: config.model,
          mode: template.agentConfig.mode,
        })
        prompts.log.success(`Agent "${agent.name}" created successfully`)
        prompts.outro(`Run: gizzi-code agent ${agent.name}`)
      } catch (err) {
        // Fallback: save config file if API unavailable
        const outputDir = (args.output as string) || process.cwd()
        const outputFile = path.join(outputDir, `${agentName.toLowerCase().replace(/\s+/g, "-")}-agent.json`)
        writeFileSync(outputFile, JSON.stringify(config, null, 2))
        prompts.log.warn(`Could not register agent via API: ${err instanceof Error ? err.message : err}`)
        prompts.log.success(`Agent configuration saved to: ${outputFile}`)
        prompts.outro("Run 'gizzi agents templates import' to register it when the server is running")
      }
    })
  },
})

// ============================================================================
// Export Command
// ============================================================================

export const AgentHubExportCommand = cmd({
  command: "export <agent-id>",
  describe: "export an agent configuration",
  builder: (yargs: Argv) => {
    return yargs
      .positional("agent-id", {
        describe: "agent ID to export",
        type: "string",
        demandOption: true,
      })
      .option("output", {
        alias: "o",
        describe: "output file path",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const agentId = args.agentId as string
      const outputFile = (args.output as string) || `agent-${agentId}.json`

      const agent = await AgentManager.get(agentId)
      if (!agent) {
        prompts.log.error(`Agent not found: ${agentId}`)
        process.exit(1)
      }

      writeFileSync(outputFile, JSON.stringify(agent, null, 2))
      prompts.log.success(`Agent "${agent.name}" exported to: ${outputFile}`)
    })
  },
})

// ============================================================================
// Import Command
// ============================================================================

export const AgentHubImportCommand = cmd({
  command: "import <file>",
  describe: "import an agent configuration",
  builder: (yargs: Argv) => {
    return yargs
      .positional("file", {
        describe: "agent configuration file to import",
        type: "string",
        demandOption: true,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const inputFile = args.file as string

      if (!existsSync(inputFile)) {
        prompts.log.error(`File not found: ${inputFile}`)
        process.exit(1)
      }

      try {
        const config = JSON.parse(readFileSync(inputFile, "utf-8"))

        prompts.intro(`Importing agent: ${config.name || "Unknown"}`)

        if (!config.name) {
          prompts.log.error("Agent config is missing required 'name' field")
          process.exit(1)
        }

        const validModes = ["subagent", "primary", "all"] as const
        const agent = await AgentManager.create({
          name: config.name,
          description: config.description,
          prompt: config.prompt,
          model: config.model,
          mode: validModes.includes(config.mode) ? config.mode : "primary",
        })
        prompts.log.success(`Agent "${agent.name}" imported successfully`)
        prompts.outro(`Run: gizzi-code agent ${agent.name}`)
      } catch (error) {
        prompts.log.error(`Failed to parse agent file: ${error}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Interactive Command (TUI)
// ============================================================================

export const AgentHubInteractiveCommand = cmd({
  command: "$0",
  describe: false,
  handler: async () => {
    await bootstrap(process.cwd(), async () => {
      prompts.intro("Agent Hub - Interactive Mode")

      // Step 1: Choose category
      const category = await prompts.select({
        message: "Select a category",
        options: [
          { value: "all", label: "All Templates" },
          { value: "engineering", label: "💻 Engineering" },
          { value: "design", label: "🎨 Design" },
          { value: "marketing", label: "📢 Marketing" },
          { value: "product", label: "📊 Product" },
          { value: "testing", label: "🧪 Testing" },
          { value: "support", label: "🛟 Support" },
          { value: "specialized", label: "🎯 Specialized" },
        ],
      })

      if (prompts.isCancel(category)) {
        prompts.cancel("Cancelled")
        return
      }

      // Filter templates
      const templates = category === "all"
        ? SPECIALIST_TEMPLATES
        : SPECIALIST_TEMPLATES.filter(t => t.category === category)

      // Step 2: Choose template
      const templateChoice = await prompts.select({
        message: "Select a template",
        options: templates.map(t => ({
          value: t.id,
          label: `${t.name} - ${t.description}`,
          hint: t.tags.slice(0, 3).join(", "),
        })),
      })

      if (prompts.isCancel(templateChoice)) {
        prompts.cancel("Cancelled")
        return
      }

      const template = getTemplateById(templateChoice as string)
      if (!template) {
        prompts.log.error("Template not found")
        return
      }

      // Step 3: Show details
      prompts.note(
        `${template.longDescription}\n\n` +
        `Success Metrics:\n` +
        template.successMetrics.slice(0, 2).map((m: any) => `  • ${m.name}: ${m.target}`).join("\n") +
        `\n\nExample: "${template.exampleInvocation}"`,
        template.name,
      )

      // Step 4: Confirm creation
      const agentName = await prompts.text({
        message: "Agent name",
        initialValue: template.name,
      })

      if (prompts.isCancel(agentName)) {
        prompts.cancel("Cancelled")
        return
      }

      const confirm = await prompts.confirm({
        message: `Create agent "${agentName}" from ${template.name} template?`,
      })

      if (prompts.isCancel(confirm) || !confirm) {
        prompts.cancel("Cancelled")
        return
      }

      // Create agent config
      const config = createAgentFromTemplate(template.id)
      
      // Save to file
      const outputFile = path.join(process.cwd(), `${(agentName as string).toLowerCase().replace(/\s+/g, "-")}-agent.json`)
      writeFileSync(outputFile, JSON.stringify(config, null, 2))

      prompts.log.success("Agent created successfully!")
      process.stdout.write(`\nConfiguration saved to: ${outputFile}\n`)
      process.stdout.write("\nNext steps:\n")
      process.stdout.write("  1. Review the configuration file\n")
      process.stdout.write("  2. Import into your agent system\n")
      process.stdout.write("  3. Start using your new specialist agent!\n")

      prompts.outro("Done!")
    })
  },
})

// ============================================================================
// Helper Functions
// ============================================================================

function formatTemplate(template: SpecialistTemplate): string {
  return `${template.name} - ${template.description} [${template.category}]`
}
