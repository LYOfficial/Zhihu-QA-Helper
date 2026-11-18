export function recoveryKey(id: string) {
  return `zhihu-qa:draft-recovery:${id}`;
}

export function clearDraftRecovery(id: string) {
  try { localStorage.removeItem(recoveryKey(id)); } catch {}
}
