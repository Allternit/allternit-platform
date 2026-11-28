import React, { useEffect, useMemo, useState } from 'react';
import { extraUsage } from './../../commands/extra-usage/index.ts';
import { Box, Text } from './../../ink.ts';
import { useClaudeAiLimits } from './../../services/claudeAiLimitsHook.ts';
import { shouldProcessMockLimits } from './../../services/rateLimitMocking.ts'; // Used for /mock-limits command
import { getRateLimitTier, getSubscriptionType, isClaudeAISubscriber } from './../../utils/auth.ts';
import { hasClaudeAiBillingAccess } from './../../utils/billing.ts';
import { MessageResponse } from '../MessageResponse';
type UpsellParams = {
  shouldShowUpsell: boolean;
  isMax20x: boolean;
  isExtraUsageCommandEnabled: boolean;
  shouldAutoOpenRateLimitOptionsMenu: boolean;
  isTeamOrEnterprise: boolean;
  hasBillingAccess: boolean;
};
export function getUpsellMessage({
  shouldShowUpsell,
  isMax20x,
  isExtraUsageCommandEnabled,
  shouldAutoOpenRateLimitOptionsMenu,
  isTeamOrEnterprise,
  hasBillingAccess
}: UpsellParams): string | null {
  if (!shouldShowUpsell) return null;
  if (isMax20x) {
    if (isExtraUsageCommandEnabled) {
      return '/extra-usage to finish what you\u2019re working on.';
    }
    return '/login to switch to an API usage-billed account.';
  }
  if (shouldAutoOpenRateLimitOptionsMenu) {
    return 'Opening your options\u2026';
  }
  if (!isTeamOrEnterprise && !isExtraUsageCommandEnabled) {
    return null;
  }
  if (isTeamOrEnterprise) {
    if (!isExtraUsageCommandEnabled) return null;
    if (hasBillingAccess) {
      return '/extra-usage to finish what you\u2019re working on.';
    }
    return '/extra-usage to request more usage from your admin.';
  }
  return '/extra-usage to finish what you\u2019re working on.';
}
type RateLimitMessageProps = {
  text: string;
  onOpenRateLimitOptions?: () => void;
};
export function RateLimitMessage({
    text,
    onOpenRateLimitOptions
}: RateLimitMessageProps) {
  const t1 = getSubscriptionType();

  const subscriptionType = t1;
  const t2 = getRateLimitTier();

  const rateLimitTier = t2;
  const isTeamOrEnterprise = subscriptionType === "team" || subscriptionType === "enterprise";
  const isMax20x = rateLimitTier === "default_claude_max_20x";
  const t3 = shouldProcessMockLimits() || isClaudeAISubscriber();

  const shouldShowUpsell = t3;
  const canSeeRateLimitOptionsUpsell = shouldShowUpsell && !isMax20x;
  const [hasOpenedInteractiveMenu, setHasOpenedInteractiveMenu] = useState(false);
  const claudeAiLimits = useClaudeAiLimits();
  const isCurrentlyRateLimited = claudeAiLimits.status === "rejected" && claudeAiLimits.resetsAt !== undefined && !claudeAiLimits.isUsingOverage;
  const shouldAutoOpenRateLimitOptionsMenu = canSeeRateLimitOptionsUpsell && !hasOpenedInteractiveMenu && isCurrentlyRateLimited && onOpenRateLimitOptions;
  const t4 = () => {
      if (shouldAutoOpenRateLimitOptionsMenu) {
        setHasOpenedInteractiveMenu(true);
        onOpenRateLimitOptions();
      }
    };
  const t5 = [shouldAutoOpenRateLimitOptionsMenu, onOpenRateLimitOptions];

  useEffect(t4, t5);
  let t6;
  bb0: {
    const t7 = getUpsellMessage({
        shouldShowUpsell,
        isMax20x,
        isExtraUsageCommandEnabled: extraUsage.isEnabled(),
        shouldAutoOpenRateLimitOptionsMenu: !!shouldAutoOpenRateLimitOptionsMenu,
        isTeamOrEnterprise,
        hasBillingAccess: hasClaudeAiBillingAccess()
      });

    const message = t7;
    if (!message) {
      t6 = null;
      break bb0;
    }
    const t8 = <Text dimColor={true}>{message}</Text>;

    t6 = t8;
  }
  const upsell = t6;
  const t7 = <Text color="error">{text}</Text>;

  const t8 = hasOpenedInteractiveMenu ? null : upsell;
  const t9 = <MessageResponse><Box flexDirection="column">{t7}{t8}</Box></MessageResponse>;

  return t9;
}
