import React, { useMemo } from 'react';
import { getMcpConfigsByScope } from './../../services/mcp/config.ts';
import type { ConfigScope } from './../../services/mcp/types.ts';
import { describeMcpConfigFilePath, getScopeLabel } from './../../services/mcp/utils.ts';
import type { ValidationError } from './../../utils/settings/validation.ts';
import { Box, Link, Text } from '../../ink';
function McpConfigErrorSection(t0) {
  const {
    scope,
    parsingErrors,
    warnings
  } = t0;
  const hasErrors = parsingErrors.length > 0;
  const hasWarnings = warnings.length > 0;
  if (!hasErrors && !hasWarnings) {
    return null;
  }
  const t1 = (hasErrors || hasWarnings) && <Text color={hasErrors ? "error" : "warning"}>[{hasErrors ? "Failed to parse" : "Contains warnings"}]{" "}</Text>;

  const t2 = getScopeLabel(scope);

  const t3 = <Text>{t2}</Text>;

  const t4 = <Box>{t1}{t3}</Box>;

  const t5 = <Text dimColor={true}>Location: </Text>;

  const t6 = describeMcpConfigFilePath(scope);

  const t7 = <Box>{t5}<Text dimColor={true}>{t6}</Text></Box>;

  const t8 = parsingErrors.map(_temp);

  const t9 = warnings.map(_temp2);

  const t10 = <Box marginLeft={1} flexDirection="column">{t8}{t9}</Box>;

  const t11 = <Box flexDirection="column" marginTop={1}>{t4}{t7}{t10}</Box>;

  return t11;
}
function _temp2(warning, i_0) {
  const serverName_0 = warning.mcpErrorMetadata?.serverName;
  return <Box key={`warning-${i_0}`}><Text><Text dimColor={true}>└ </Text><Text color="warning">[Warning]</Text><Text dimColor={true}>{" "}{serverName_0 && `[${serverName_0}] `}{warning.path && warning.path !== "" ? `${warning.path}: ` : ""}{warning.message}</Text></Text></Box>;
}
function _temp(error, i) {
  const serverName = error.mcpErrorMetadata?.serverName;
  return <Box key={`error-${i}`}><Text><Text dimColor={true}>└ </Text><Text color="error">[Error]</Text><Text dimColor={true}>{" "}{serverName && `[${serverName}] `}{error.path && error.path !== "" ? `${error.path}: ` : ""}{error.message}</Text></Text></Box>;
}
export function McpParsingWarnings() {
  const t0 = {
      scope: "user" as ConfigScope,
      config: getMcpConfigsByScope("user")
    };

  const t1 = {
      scope: "project" as ConfigScope,
      config: getMcpConfigsByScope("project")
    };

  const t2 = {
      scope: "local" as ConfigScope,
      config: getMcpConfigsByScope("local")
    };

  const t3 = [t0, t1, t2, {
      scope: "enterprise" as ConfigScope,
      config: getMcpConfigsByScope("enterprise")
    }];

  const scopes = t3 satisfies Array<{
    scope: ConfigScope;
    config: {
      errors: ValidationError[];
    };
  }>;
  const hasParsingErrors = scopes.some(_temp3);
  const hasWarnings = scopes.some(_temp4);
  if (!hasParsingErrors && !hasWarnings) {
    return null;
  }
  const t4 = <Text bold={true}>MCP Config Diagnostics</Text>;

  const t5 = <Box flexDirection="column" marginTop={1} marginBottom={1}>{t4}<Box marginTop={1}><Text dimColor={true}>For help configuring MCP servers, see:{" "}<Link url="https://docs.allternit.com/cli/mcp">https://docs.allternit.com/cli/mcp</Link></Text></Box>{scopes.map(_temp5)}</Box>;
  return t5;
}
function _temp5(t0) {
  const {
    scope,
    config: config_1
  } = t0;
  return <McpConfigErrorSection key={scope} scope={scope} parsingErrors={filterErrors(config_1.errors, "fatal")} warnings={filterErrors(config_1.errors, "warning")} />;
}
function _temp4(t0) {
  const {
    config: config_0
  } = t0;
  return filterErrors(config_0.errors, "warning").length > 0;
}
function _temp3(t0) {
  const {
    config
  } = t0;
  return filterErrors(config.errors, "fatal").length > 0;
}
function filterErrors(errors: ValidationError[], severity: 'fatal' | 'warning'): ValidationError[] {
  return errors.filter(e => e.mcpErrorMetadata?.severity === severity);
}
