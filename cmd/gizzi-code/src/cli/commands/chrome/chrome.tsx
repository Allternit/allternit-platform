import React, { useState } from 'react';
import { type OptionWithDescription, Select } from '../../ui/ink-app/components/CustomSelect/select.js';
import { Dialog } from '../../ui/ink-app/components/design-system/Dialog.js';
import { Box, Text } from '@/ink.js';
import { useAppState } from '@/state/AppState.js';
import { isClaudeAISubscriber } from '../../utils/auth.js';
import { openBrowser } from '../../../shared/utils/browser.js';
import { ALLTERNIT_IN_CHROME_MCP_SERVER_NAME, openInChrome } from '../../utils/allternitInChrome/common.js';
import { isChromeExtensionInstalled } from '../../utils/allternitInChrome/setup.js';
import { getGlobalConfig, saveGlobalConfig } from '../../../shared/utils/config.js';
import { env } from '../../../shared/utils/env.js';
import { isRunningOnHomespace } from '../../../shared/utils/envUtils.js';
const CHROME_EXTENSION_URL = 'https://chromewebstore.google.com/detail/chheieepkpbhkiimdmbdjmnhcooclpok';
const CHROME_PERMISSIONS_URL = 'chrome://extensions/?id=chheieepkpbhkiimdmbdjmnhcooclpok';
const CHROME_RECONNECT_URL = 'chrome-extension://chheieepkpbhkiimdmbdjmnhcooclpok/';
type MenuAction = 'install-extension' | 'reconnect' | 'manage-permissions' | 'toggle-default';
type Props = {
  onDone: (result?: string) => void;
  isExtensionInstalled: boolean;
  configEnabled: boolean | undefined;
  isClaudeAISubscriber: boolean;
  isWSL: boolean;
};
function AllternitInChromeMenu({
    onDone,
    isExtensionInstalled: installed,
    configEnabled,
    isClaudeAISubscriber,
    isWSL
}: Props) {
  const mcpClients = useAppState(_temp);
  const [selectKey, setSelectKey] = useState(0);
  const [enabledByDefault, setEnabledByDefault] = useState(configEnabled ?? false);
  const [showInstallHint, setShowInstallHint] = useState(false);
  const [isExtensionInstalled, setIsExtensionInstalled] = useState(installed);
  const t1 = false && isRunningOnHomespace();

  const isHomespace = t1;
  const t2 = mcpClients.find(_temp2);

  const chromeClient = t2;
  const isConnected = chromeClient?.type === "connected";
  const t3 = function openUrl(url) {
      if (isHomespace) {
        openBrowser(url);
      } else {
        openInChrome(url);
      }
    };

  const openUrl = t3;
  const t4 = function handleAction(action) {
      bb22: switch (action) {
        case "install-extension":
          {
            setSelectKey(_temp3);
            setShowInstallHint(true);
            openUrl(CHROME_EXTENSION_URL);
            break bb22;
          }
        case "reconnect":
          {
            setSelectKey(_temp4);
            isChromeExtensionInstalled().then(installed_0 => {
              setIsExtensionInstalled(installed_0);
              if (installed_0) {
                setShowInstallHint(false);
              }
            });
            openUrl(CHROME_RECONNECT_URL);
            break bb22;
          }
        case "manage-permissions":
          {
            setSelectKey(_temp5);
            openUrl(CHROME_PERMISSIONS_URL);
            break bb22;
          }
        case "toggle-default":
          {
            const newValue = !enabledByDefault;
            saveGlobalConfig(current => ({
              ...current,
              allternitInChromeDefaultEnabled: newValue
            }));
            setEnabledByDefault(newValue);
          }
      }
    };

  const handleAction = t4;
  const options: OptionWithDescription[] = [];
  const requiresExtensionSuffix = isExtensionInstalled ? "" : " (requires extension)";
  if (!isExtensionInstalled && !isHomespace) {
      const t5 = {
          label: "Install Chrome extension",
          value: "install-extension"
        };

      options.push(t5);
    }
  const t5 = <Text>Manage permissions</Text>;

  const t6 = {
        label: <>{t5}<Text dimColor={true}>{requiresExtensionSuffix}</Text></>,
        value: "manage-permissions"
      };

  const t7 = <Text>Reconnect extension</Text>;

  const t8 = {
        label: <>{t7}<Text dimColor={true}>{requiresExtensionSuffix}</Text></>,
        value: "reconnect"
      };

  const t9 = `Enabled by default: ${enabledByDefault ? "Yes" : "No"}`;
  const t10 = {
        label: t9,
        value: "toggle-default"
      };

  options.push(t6, t8, t10);

  const isDisabled = isWSL;
  const t5_2 = () => onDone();

  const t6_2 = <Text>Allternit in Chrome works with the Chrome extension to let you control your browser directly from Gizzi. Navigate websites, fill forms, capture screenshots, record GIFs, and debug with console logs and network requests.</Text>;

  const t7_2 = isWSL && <Text color="error">Allternit in Chrome is not supported in WSL at this time.</Text>;

  let t8_2 = null;
  const t9_2 = !isDisabled && <>{!isHomespace && <Box flexDirection="column"><Text>Status:{" "}{isConnected ? <Text color="success">Enabled</Text> : <Text color="inactive">Disabled</Text>}</Text><Text>Extension:{" "}{isExtensionInstalled ? <Text color="success">Installed</Text> : <Text color="warning">Not detected</Text>}</Text></Box>}<Select key={selectKey} options={options} onChange={handleAction} hideIndexes={true} />{showInstallHint && <Text color="warning">Once installed, select {"\"Reconnect extension\""} to connect.</Text>}<Text><Text dimColor={true}>Usage: </Text><Text>gizzi --chrome</Text><Text dimColor={true}> or </Text><Text>gizzi --no-chrome</Text></Text><Text dimColor={true}>Site-level permissions are inherited from the Chrome extension. Manage permissions in the Chrome extension settings to control which sites Gizzi can browse, click, and type on.</Text></>;

  const t10_2 = <Text dimColor={true}>Learn more: https://github.com/Allternit/desktop</Text>;

  const t11 = <Box flexDirection="column" gap={1}>{t6_2}{t7_2}{t8_2}{t9_2}{t10_2}</Box>;

  const t12 = <Dialog title="Allternit in Chrome (Beta)" onCancel={t5_2} color="chromeYellow">{t11}</Dialog>;

  return t12;
}
function _temp5(k) {
  return k + 1;
}
function _temp4(k_0) {
  return k_0 + 1;
}
function _temp3(k_1) {
  return k_1 + 1;
}
function _temp2(c) {
  return c.name === ALLTERNIT_IN_CHROME_MCP_SERVER_NAME;
}
function _temp(s) {
  return s.mcp.clients;
}
export const call = async function (onDone: (result?: string) => void): Promise<React.ReactNode> {
  const isExtensionInstalled = await isChromeExtensionInstalled();
  const config = getGlobalConfig();
  const isSubscriber = await isClaudeAISubscriber();
  const isWSL = env.isWslEnvironment();
  return <AllternitInChromeMenu onDone={onDone} isExtensionInstalled={isExtensionInstalled} configEnabled={config.allternitInChromeDefaultEnabled} isClaudeAISubscriber={isSubscriber} isWSL={isWSL} />;
};
