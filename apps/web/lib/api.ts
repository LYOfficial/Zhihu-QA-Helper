/**
 * 数据访问层。只连本地内核（Rust 服务，127.0.0.1:8787）。
 * 内核没起来时不做任何伪装：直接进入 offline 状态，由界面给出明确提示。
 */
import type { Job } from "./types";

export type Mode = "unknown" | "live" | "offline";

let mode: Mode = "unknown";
let cachedBase: string | null = null;
const modeListeners = new Set<(m: Mode) => void>();

export function getMode() {
  return mode;
}

export function onModeChange(fn: (m: Mode) => void) {
  modeListeners.add(fn);
  return () => {
    modeListeners.delete(fn);
  };
}

function setMode(m: Mode) {
  if (mode === m) return;
  mode = m;
  modeListeners.forEach((f) => f(m));
}

export function apiBase(): string {
  if (typeof window === "undefined") return "";
  if (cachedBase !== null) return cachedBase;
  const env = process.env.NEXT_PUBLIC_API_BASE;
  if (env) {
    cachedBase = env.replace(/\/$/, "");
  } else if (window.location.port === "31873") {
    // next dev：前端在 31873，内核在 8787
    cachedBase = "http://127.0.0.1:8787";
  } else {
    cachedBase = ""; // 桌面壳内：内核托管静态资源，同源
  }
  return cachedBase;
}

export async function probe(): Promise<Mode> {
  try {
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort(), 1500);
    const res = await fetch(`${apiBase()}/api/health`, { signal: ctl.signal });
    clearTimeout(timer);
    if (!res.ok) throw new Error("bad status");
    await res.json();
    setMode("live");
  } catch {
    setMode("offline");
  }
  return mode;
}

export function imageSrc(questionId: string, filename: string): string {
  return `${apiBase()}/api/images/${encodeURIComponent(questionId)}/${encodeURIComponent(filename)}`;
}

export function openExternal(raw: string): boolean {
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("unsupported");
    window.open(url.toString(), "_blank", "noopener,noreferrer");
    return true;
  } catch {
    toast("这个问题没有可用的知乎链接", "err");
    return false;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  if (mode === "unknown") await probe();
  if (mode === "offline") {
    throw new Error("本地内核没有响应。请确认软件主程序仍在运行。");
  }
  const res = await fetch(`${apiBase()}${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", ...(init?.headers || {}) },
  });
  const text = await res.text();
  let data: unknown = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    throw new Error(`服务返回了非 JSON 内容：${text.slice(0, 120)}`);
  }
  if (!res.ok) {
    const msg = (data as { error?: string })?.error || `HTTP ${res.status}`;
    throw new Error(msg);
  }
  return data as T;
}

export const api = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "POST", body: JSON.stringify(body ?? {}) }),
  put: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "PUT", body: JSON.stringify(body ?? {}) }),
  del: <T>(path: string) => request<T>(path, { method: "DELETE" }),
};

// ---------------------------------------------------------------- 提示

type ToastFn = (msg: string, kind?: "ok" | "err" | "info") => void;
let toastFn: ToastFn | null = null;
export function registerToast(fn: ToastFn) {
  toastFn = fn;
}
export function toast(msg: string, kind: "ok" | "err" | "info" = "info") {
  toastFn?.(msg, kind);
}

export type { Job };
