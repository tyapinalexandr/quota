import { invoke } from '@tauri-apps/api/core';

export interface KimiAccountSummary {
  id: string;
  label: string;
  keyHint?: string | null;
  /** Share of the monthly quota still available, 0-100. */
  monthlyRemainingPercent?: number | null;
  /** Share of the monthly quota used, 0-100. */
  monthlyUsedPercent?: number | null;
  /** Unix seconds when the monthly quota resets. */
  monthlyResetAt?: number | null;
  /** Share of the 5-hour window still available, 0-100. */
  fiveHourRemainingPercent?: number | null;
  /** Unix seconds when the 5-hour window resets. */
  fiveHourResetAt?: number | null;
  /** Requests left in the short rate window. */
  rateLimitRemaining?: number | null;
  rateLimitLimit?: number | null;
  /** Unix seconds when the rate window resets. */
  rateLimitResetAt?: number | null;
  /** Subscription level name from the Kimi profile (e.g. "Max"). */
  userLevelName?: string | null;
  quotaQueryLastError?: string | null;
  usageUpdatedAt?: number | null;
  createdAt: number;
  lastUsed: number;
}

export function listKimiAccounts() {
  return invoke<KimiAccountSummary[]>('list_kimi_accounts');
}

export function startKimiAddKey(apiKey: string, label?: string) {
  return invoke<string>('kimi_add_key_start', { apiKey, label: label ?? null });
}

export function completeKimiAddKey(loginId: string) {
  return invoke<KimiAccountSummary>('kimi_add_key_complete', { loginId });
}

export function cancelKimiAddKey(loginId?: string | null) {
  return invoke<void>('kimi_add_key_cancel', { loginId: loginId ?? null });
}

export function refreshKimiAccount(accountId: string) {
  return invoke<KimiAccountSummary>('refresh_kimi_account', { accountId });
}

export function refreshAllKimiAccounts() {
  return invoke<KimiAccountSummary[]>('refresh_all_kimi_accounts');
}

export function deleteKimiAccount(accountId: string) {
  return invoke<void>('delete_kimi_account', { accountId });
}
