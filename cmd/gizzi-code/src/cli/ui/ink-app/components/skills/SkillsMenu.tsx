import capitalize from 'lodash-es/capitalize';
import * as React from 'react';
import { useMemo } from 'react';
import { type Command, type CommandBase, type CommandResultDisplay, getCommandName, type PromptCommand } from '../../commands';
import { Box, Text } from '../../ink';
import { estimateSkillFrontmatterTokens, getSkillsPath } from '../../skills/loadSkillsDir';
import { getDisplayPath } from '../../utils/file';
import { formatTokens } from '../../utils/format';
import { getSettingSourceName, type SettingSource } from '../../utils/settings/constants';
import { plural } from '../../utils/stringUtils';
import { ConfigurableShortcutHint } from '../ConfigurableShortcutHint';
import { Dialog } from '../design-system/Dialog';

// Skills are always PromptCommands with CommandBase properties
type SkillCommand = CommandBase & PromptCommand;
type SkillSource = SettingSource | 'plugin' | 'mcp';
type Props = {
  onExit: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
  commands: Command[];
};
function getSourceTitle(source: SkillSource): string {
  if (source === 'plugin') {
    return 'Plugin skills';
  }
  if (source === 'mcp') {
    return 'MCP skills';
  }
  return `${capitalize(getSettingSourceName(source))} skills`;
}
function getSourceSubtitle(source: SkillSource, skills: SkillCommand[]): string | undefined {
  // MCP skills show server names; file-based skills show filesystem paths.
  // Skill names are `<server>:<skill>`, not `mcp__<server>__…`.
  if (source === 'mcp') {
    const servers = [...new Set(skills.map(s => {
      const idx = s.name.indexOf(':');
      return idx > 0 ? s.name.slice(0, idx) : null;
    }).filter((n): n is string => n != null))];
    return servers.length > 0 ? servers.join(', ') : undefined;
  }
  const skillsPath = getDisplayPath(getSkillsPath(source, 'skills'));
  const hasCommandsSkills = skills.some(s => s.loadedFrom === 'commands_DEPRECATED');
  return hasCommandsSkills ? `${skillsPath}, ${getDisplayPath(getSkillsPath(source, 'commands'))}` : skillsPath;
}
export function SkillsMenu({
    onExit,
    commands
}: Props) {
  const t1 = commands.filter(_temp);

  const skills = t1;
  const groups = {
      policySettings: [],
      userSettings: [],
      projectSettings: [],
      localSettings: [],
      flagSettings: [],
      plugin: [],
      mcp: []
    };
  for (const skill of skills) {
      const source = skill.source as SkillSource;
      if (source in groups) {
        groups[source].push(skill);
      }
    }
  for (const group of Object.values(groups)) {
      group.sort(_temp2);
    }

  const skillsBySource = groups;
  const t2 = () => {
      onExit("Skills dialog dismissed", {
        display: "system"
      });
    };

  const handleCancel = t2;
  if (skills.length === 0) {
    const t3 = <Text dimColor={true}>Create skills in .claude/skills/ or ~/.gizzi/skills/</Text>;

    const t4 = <Text dimColor={true} italic={true}><ConfigurableShortcutHint action="confirm:no" context="Confirmation" fallback="Esc" description="close" /></Text>;

    const t5 = <Dialog title="Skills" subtitle="No skills found" onCancel={handleCancel} hideInputGuide={true}>{t3}{t4}</Dialog>;

    return t5;
  }
  const renderSkill = _temp3;
  const t3 = source_0 => {
      const groupSkills = skillsBySource[source_0];
      if (groupSkills.length === 0) {
        return null;
      }
      const title = getSourceTitle(source_0);
      const subtitle = getSourceSubtitle(source_0, groupSkills);
      return <Box flexDirection="column" key={source_0}><Box><Text bold={true} dimColor={true}>{title}</Text>{subtitle && <Text dimColor={true}> ({subtitle})</Text>}</Box>{groupSkills.map(skill_1 => renderSkill(skill_1))}</Box>;
    };

  const renderSkillGroup = t3;
  const t4 = skills.length;
  const t5 = plural(skills.length, "skill");

  const t6 = `${t4} ${t5}`;
  const t7 = renderSkillGroup("projectSettings");

  const t8 = renderSkillGroup("userSettings");

  const t9 = renderSkillGroup("policySettings");

  const t10 = renderSkillGroup("plugin");

  const t11 = renderSkillGroup("mcp");

  const t12 = <Box flexDirection="column" gap={1}>{t7}{t8}{t9}{t10}{t11}</Box>;

  const t13 = <Text dimColor={true} italic={true}><ConfigurableShortcutHint action="confirm:no" context="Confirmation" fallback="Esc" description="close" /></Text>;

  const t14 = <Dialog title="Skills" subtitle={t6} onCancel={handleCancel} hideInputGuide={true}>{t12}{t13}</Dialog>;

  return t14;
}
function _temp3(skill_0: SkillCommand) {
  const estimatedTokens = estimateSkillFrontmatterTokens(skill_0);
  const tokenDisplay = `~${formatTokens(estimatedTokens)}`;
  const pluginName = skill_0.source === "plugin" ? skill_0.pluginInfo?.pluginManifest.name : undefined;
  return <Box key={`${skill_0.name}-${skill_0.source}`}><Text>{getCommandName(skill_0)}</Text><Text dimColor={true}>{pluginName ? ` · ${pluginName}` : ""} · {tokenDisplay} description tokens</Text></Box>;
}
function _temp2(a: SkillCommand, b: SkillCommand) {
  return getCommandName(a).localeCompare(getCommandName(b));
}
function _temp(cmd: Command): cmd is SkillCommand {
  return cmd.type === "prompt" && (cmd.loadedFrom === "skills" || cmd.loadedFrom === "commands_DEPRECATED" || cmd.loadedFrom === "plugin" || cmd.loadedFrom === "mcp");
}
