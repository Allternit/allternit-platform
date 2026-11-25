import figures from 'figures';
import * as React from 'react';
import type { SettingSource } from './../../utils/settings/constants.ts';
import type { KeyboardEvent } from '../../ink/events/keyboard-event';
import { Box, Text } from '../../ink';
import type { ResolvedAgent } from '../../tools/AgentTool/agentDisplay';
import { AGENT_SOURCE_GROUPS, compareAgentsByName, getOverrideSourceLabel, resolveAgentModelDisplay } from '../../tools/AgentTool/agentDisplay';
import type { AgentDefinition } from '../../tools/AgentTool/loadAgentsDir';
import { count } from '../../utils/array';
import { Dialog } from '../design-system/Dialog';
import { Divider } from '../design-system/Divider';
import { getAgentSourceDisplayName } from './utils';
import { EmptyLine } from '../EmptyLine'
type Props = {
  source: SettingSource | 'all' | 'built-in' | 'plugin';
  agents: ResolvedAgent[];
  onBack: () => void;
  onSelect: (agent: AgentDefinition) => void;
  onCreateNew?: () => void;
  changes?: string[];
};
export function AgentsList({
    source,
    agents,
    onBack,
    onSelect,
    onCreateNew,
    changes
}: Props) {
  const [selectedAgent, setSelectedAgent] = React.useState<ResolvedAgent | null>(null);
  const [isCreateNewSelected, setIsCreateNewSelected] = React.useState(true);
  const t1 = [...agents].sort(compareAgentsByName);

  const sortedAgents = t1;
  const getOverrideInfo = _temp;
  const t2 = () => <Box><Text color={isCreateNewSelected ? "suggestion" : undefined}>{isCreateNewSelected ? `${figures.pointer} ` : "  "}</Text><Text color={isCreateNewSelected ? "suggestion" : undefined}>Create new agent</Text></Box>;

  const renderCreateNewOption = t2;
  const t3 = agent_0 => {
      const isBuiltIn = agent_0.source === "built-in";
      const isSelected = !isBuiltIn && !isCreateNewSelected && selectedAgent?.agentType === agent_0.agentType && selectedAgent?.source === agent_0.source;
      const {
        isOverridden,
        overriddenBy
      } = getOverrideInfo(agent_0);
      const dimmed = isBuiltIn || isOverridden;
      const textColor = !isBuiltIn && isSelected ? "suggestion" : undefined;
      const resolvedModel = resolveAgentModelDisplay(agent_0);
      return <Box key={`${agent_0.agentType}-${agent_0.source}`}><Text dimColor={dimmed && !isSelected} color={textColor}>{isBuiltIn ? "" : isSelected ? `${figures.pointer} ` : "  "}</Text><Text dimColor={dimmed && !isSelected} color={textColor}>{agent_0.agentType}</Text>{resolvedModel && <Text dimColor={true} color={textColor}>{" \xB7 "}{resolvedModel}</Text>}{agent_0.memory && <Text dimColor={true} color={textColor}>{" \xB7 "}{agent_0.memory} memory</Text>}{overriddenBy && <Text dimColor={!isSelected} color={isSelected ? "warning" : undefined}>{" "}{figures.warning} shadowed by {getOverrideSourceLabel(overriddenBy)}</Text>}</Box>;
    };

  const renderAgent = t3;
  let t4;
  bb0: {
    const nonBuiltIn = sortedAgents.filter(_temp2);
    if (source === "all") {
      t4 = AGENT_SOURCE_GROUPS.filter(_temp3).flatMap(t5 => {
        const {
          source: groupSource
        } = t5;
        return nonBuiltIn.filter(a_0 => a_0.source === groupSource);
      });
      break bb0;
    }
    t4 = nonBuiltIn;
  }
  

  const selectableAgentsInOrder = t4;
  const t5 = () => {
      if (!selectedAgent && !isCreateNewSelected && selectableAgentsInOrder.length > 0) {
        if (onCreateNew) {
          setIsCreateNewSelected(true);
        } else {
          setSelectedAgent(selectableAgentsInOrder[0] || null);
        }
      }
    };
  const t6 = [selectableAgentsInOrder, selectedAgent, isCreateNewSelected, onCreateNew];

  React.useEffect(t5, t6);
  const t7 = e => {
      if (e.key === "return") {
        e.preventDefault();
        if (isCreateNewSelected && onCreateNew) {
          onCreateNew();
        } else {
          if (selectedAgent) {
            onSelect(selectedAgent);
          }
        }
        return;
      }
      if (e.key !== "up" && e.key !== "down") {
        return;
      }
      e.preventDefault();
      const hasCreateOption = !!onCreateNew;
      const totalItems = selectableAgentsInOrder.length + (hasCreateOption ? 1 : 0);
      if (totalItems === 0) {
        return;
      }
      let currentPosition = 0;
      if (!isCreateNewSelected && selectedAgent) {
        const agentIndex = selectableAgentsInOrder.findIndex(a_1 => a_1.agentType === selectedAgent.agentType && a_1.source === selectedAgent.source);
        if (agentIndex >= 0) {
          currentPosition = hasCreateOption ? agentIndex + 1 : agentIndex;
        }
      }
      const newPosition = e.key === "up" ? currentPosition === 0 ? totalItems - 1 : currentPosition - 1 : currentPosition === totalItems - 1 ? 0 : currentPosition + 1;
      if (hasCreateOption && newPosition === 0) {
        setIsCreateNewSelected(true);
        setSelectedAgent(null);
      } else {
        const agentIndex_0 = hasCreateOption ? newPosition - 1 : newPosition;
        const newAgent = selectableAgentsInOrder[agentIndex_0];
        if (newAgent) {
          setIsCreateNewSelected(false);
          setSelectedAgent(newAgent);
        }
      }
    };

  const handleKeyDown = t7;
  const t8 = (t9 = "Built-in (always available):") => {
      const title = t9 === undefined ? "Built-in (always available):" : t9;
      const builtInAgents = sortedAgents.filter(_temp4);
      return <Box flexDirection="column" marginBottom={1} paddingLeft={2}><Text bold={true} dimColor={true}>{title}</Text>{builtInAgents.map(renderAgent)}</Box>;
    };

  const renderBuiltInAgentsSection = t8;
  const t9 = (title_0, groupAgents) => {
      if (!groupAgents.length) {
        return null;
      }
      const folderPath = groupAgents[0]?.baseDir;
      return <Box flexDirection="column" marginBottom={1}><Box paddingLeft={2}><Text bold={true} dimColor={true}>{title_0}</Text>{folderPath && <Text dimColor={true}> ({folderPath})</Text>}</Box>{groupAgents.map(agent_1 => renderAgent(agent_1))}</Box>;
    };

  const renderAgentGroup = t9;
  const t10 = getAgentSourceDisplayName(source);

  const sourceTitle = t10;
  let T0;
  let T1;
  let t11;
  let t12;
  let t13;
  let t14;
  let t15;
  let t16;
  let t17;
  let t18;
  let t19;
  let t20;
  let t21;
  let t22;
  t22 = Symbol.for("react.early_return_sentinel");
  bb1: {
    const builtInAgents_0 = sortedAgents.filter(_temp5);
    const hasNoAgents = !sortedAgents.length || source !== "built-in" && !sortedAgents.some(_temp6);
    if (hasNoAgents) {
      const t23 = onCreateNew && <Box>{renderCreateNewOption()}</Box>;

      const t24 = <EmptyLine>No agents found. Create specialized subagents that Gizzi can delegate to.</EmptyLine>;
      const t25 = <Text dimColor={true}>Each subagent has its own context window, custom system prompt, and specific tools.</Text>;
      const t26 = <Text dimColor={true}>Try creating: Code Reviewer, Code Simplifier, Security Reviewer, Tech Lead, or UX Reviewer.</Text>;

      const t27 = source !== "built-in" && sortedAgents.some(_temp7) && <><Divider />{renderBuiltInAgentsSection()}</>;

      const t28 = <Box flexDirection="column" gap={1} tabIndex={0} autoFocus={true} onKeyDown={handleKeyDown}>{t23}{t24}{t25}{t26}{t27}</Box>;

      const t29 = <Dialog title={sourceTitle} subtitle="No agents found" onCancel={onBack} hideInputGuide={true}>{t28}</Dialog>;

      t22 = t29;
      break bb1;
    }
    T1 = Dialog;
    t17 = sourceTitle;
    const t23 = count(sortedAgents, _temp8);

    t18 = `${t23} agents`;
    t19 = onBack;
    t20 = true;

      t21 = changes && changes.length > 0 && <Box marginTop={1}><Text dimColor={true}>{changes[changes.length - 1]}</Text></Box>;
    
    T0 = Box;
    t11 = "column";
    t12 = 0;
    t13 = true;
    t14 = handleKeyDown;

      t15 = onCreateNew && <Box marginBottom={1}>{renderCreateNewOption()}</Box>;
    
    t16 = source === "all" ? <>{AGENT_SOURCE_GROUPS.filter(_temp9).map(t24 => {
        const {
          label,
          source: groupSource_0
        } = t24;
        return <React.Fragment key={groupSource_0}>{renderAgentGroup(label, sortedAgents.filter(a_7 => a_7.source === groupSource_0))}</React.Fragment>;
      })}{builtInAgents_0.length > 0 && <Box flexDirection="column" marginBottom={1} paddingLeft={2}><Text dimColor={true}><Text bold={true}>Built-in agents</Text> (always available)</Text>{builtInAgents_0.map(renderAgent)}</Box>}</> : source === "built-in" ? <><Text dimColor={true} italic={true}>Built-in agents are provided by default and cannot be modified.</Text><Box marginTop={1} flexDirection="column">{sortedAgents.map(agent_2 => renderAgent(agent_2))}</Box></> : <>{sortedAgents.filter(_temp0).map(agent_3 => renderAgent(agent_3))}{sortedAgents.some(_temp1) && <><Divider />{renderBuiltInAgentsSection()}</>}</>;
  }
  

  if (t22 !== Symbol.for("react.early_return_sentinel")) {
    return t22;
  }
  const t23 = <T0 flexDirection={t11} tabIndex={t12} autoFocus={t13} onKeyDown={t14}>{t15}{t16}</T0>;

  const t24 = <T1 title={t17} subtitle={t18} onCancel={t19} hideInputGuide={t20}>{t21}{t23}</T1>;

  return t24;
}
function _temp1(a_9) {
  return a_9.source === "built-in";
}
function _temp0(a_8) {
  return a_8.source !== "built-in";
}
function _temp9(g_0) {
  return g_0.source !== "built-in";
}
function _temp8(a_6) {
  return !a_6.overriddenBy;
}
function _temp7(a_5) {
  return a_5.source === "built-in";
}
function _temp6(a_4) {
  return a_4.source !== "built-in";
}
function _temp5(a_3) {
  return a_3.source === "built-in";
}
function _temp4(a_2) {
  return a_2.source === "built-in";
}
function _temp3(g) {
  return g.source !== "built-in";
}
function _temp2(a) {
  return a.source !== "built-in";
}
function _temp(agent) {
  return {
    isOverridden: !!agent.overriddenBy,
    overriddenBy: agent.overriddenBy || null
  };
}
