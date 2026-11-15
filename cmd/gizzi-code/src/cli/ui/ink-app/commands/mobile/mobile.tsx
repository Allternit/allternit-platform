import { toString as qrToString } from 'qrcode';
import * as React from 'react';
import { useCallback, useEffect, useState } from 'react';
import { Pane } from '../../components/design-system/Pane';
import type { KeyboardEvent } from '../../ink/events/keyboard-event';
import { Box, Text } from '../../ink';
import { useKeybinding } from '../../keybindings/useKeybinding';
import type { LocalJSXCommandOnDone } from '../../types/command';
type Platform = 'ios' | 'android';
type Props = {
  onDone: () => void;
};
const PLATFORMS: Record<Platform, {
  url: string;
}> = {
  // Same links as allternit-ai src/lib/store-links.ts and the www site.
  ios: {
    url: 'https://apps.apple.com/app/id6828841999'
  },
  android: {
    url: 'https://play.google.com/store/apps/details?id=com.allternit.app'
  }
};
function MobileQRCode({
    onDone
}: Props) {
  const [platform, setPlatform] = useState("ios");
  const t1 = {
      ios: "",
      android: ""
    };

  const [qrCodes, setQrCodes] = useState(t1);
  const {
    url
  } = PLATFORMS[platform];
  const qrCode = qrCodes[platform];
  const t2 = () => {
      const generateQRCodes = async function generateQRCodes() {
        const [ios, android] = await Promise.all([qrToString(PLATFORMS.ios.url, {
          type: "utf8",
          errorCorrectionLevel: "L"
        }), qrToString(PLATFORMS.android.url, {
          type: "utf8",
          errorCorrectionLevel: "L"
        })]);
        setQrCodes({
          ios,
          android
        });
      };
      generateQRCodes().catch(_temp);
    };
  const t3 = [];

  useEffect(t2, t3);
  const t4 = () => {
      onDone();
    };

  const handleClose = t4;
  const t5 = {
      context: "Confirmation"
    };

  useKeybinding("confirm:no", handleClose, t5);
  const t6 = function handleKeyDown(e) {
      if (e.key === "q" || e.ctrl && e.key === "c") {
        e.preventDefault();
        onDone();
        return;
      }
      if (e.key === "tab" || e.key === "left" || e.key === "right") {
        e.preventDefault();
        setPlatform(_temp2);
      }
    };

  const handleKeyDown = t6;
  let T0;
  let T1;
  let t10;
  let t11;
  let t12;
  let t13;
  let t7;
  let t8;
  let t9;
  const lines = qrCode.split("\n").filter(_temp3);
  T1 = Pane;
  T0 = Box;
  t7 = "column";
  t8 = 0;
  t9 = true;
  t10 = handleKeyDown;

    t11 = <Text> </Text>;
    t12 = <Text> </Text>;
  
  t13 = lines.map(_temp4);
  

  const t14 = <Text> </Text>;
  const t15 = <Text> </Text>;

  const t16 = platform === "ios";
  const t17 = platform === "ios";
  const t18 = <Text bold={t16} underline={t17}>iOS</Text>;

  const t19 = <Text dimColor={true}>{" / "}</Text>;

  const t20 = platform === "android";
  const t21 = platform === "android";
  const t22 = <Text bold={t20} underline={t21}>Android</Text>;

  const t23 = <Text>{t18}{t19}{t22}</Text>;

  const t24 = <Text dimColor={true}>(tab to switch, esc to close)</Text>;

  const t25 = <Box flexDirection="row" gap={2}>{t23}{t24}</Box>;

  const t26 = <Text dimColor={true}>{url}</Text>;

  const t27 = <T0 flexDirection={t7} tabIndex={t8} autoFocus={t9} onKeyDown={t10}>{t11}{t12}{t13}{t14}{t15}{t25}{t26}</T0>;

  const t28 = <T1>{t27}</T1>;

  return t28;
}
function _temp4(line_0, i) {
  return <Text key={i}>{line_0}</Text>;
}
function _temp3(line) {
  return line.length > 0;
}
function _temp2(prev) {
  return prev === "ios" ? "android" : "ios";
}
function _temp() {}
export async function call(onDone: LocalJSXCommandOnDone): Promise<React.ReactNode> {
  return <MobileQRCode onDone={onDone} />;
}
