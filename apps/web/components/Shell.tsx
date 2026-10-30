"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import React from "react";
import {
  getMode,
  onModeChange,
  probe,
  registerToast,
  type Mode,
} from "@/lib/api";
import { api } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import { getTheme, saveTheme, type Theme } from "@/lib/theme";
import type { Health, Session, StatsOverview } from "@/lib/types";
import LoginScreen from "./LoginScreen";
import {
  IconChart,
  IconDesk,
  IconGlobe,
  IconInbox,
  IconQuill,
  IconSliders,
  IconSpark,
  IconUser,
  Seal,
} from "./ui";

type Toast = { id: number; msg: string; kind: "ok" | "err" | "info" };

const GROUPS = [
  {
    label: "工作台",
    items: [
      { href: "/", label: "案头", icon: IconDesk, badge: "" },
      { href: "/inbox", label: "奏章", icon: IconInbox, badge: "inbox" },
      { href: "/desk", label: "批阅台", icon: IconQuill, badge: "pending" },
    ],
  },
  {
    label: "配置",
    items: [
      { href: "/profile", label: "领域画像", icon: IconSpark, badge: "" },
      { href: "/sources", label: "权威平台", icon: IconGlobe, badge: "" },
      { href: "/stats", label: "台账", icon: IconChart, badge: "" },
      { href: "/settings", label: "设置", icon: IconSliders, badge: "" },
    ],
  },
];

function Boot({ text }: { text: string }) {
  return (
    <div className="boot">
      <div className="boot-inner">
        <span className="mark">
          <Seal size={40} />
        </span>
        <div>{text}</div>
      </div>
    </div>
  );
}

function Offline({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="boot">
      <div className="offline-card">
        <h2>内核没有响应</h2>
        <p>
          界面进程还活着，但本地内核（127.0.0.1:8787）没有应答。
          常见原因是内核被安全软件拦下，或者同时开了多个实例。
        </p>
        <button className="btn btn-primary" onClick={onRetry}>
          重新连接
        </button>
        <p style={{ marginTop: 14, fontSize: 11, color: "var(--ink-4)" }}>
          仍然失败请退出软件重开；日志在 data/logs/core.log
        </p>
      </div>
    </div>
  );
}

export default function Shell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const [mode, setMode] = React.useState<Mode>(getMode());
  const [theme, setTheme] = React.useState<Theme>("zhihu");
  const [toasts, setToasts] = React.useState<Toast[]>([]);
  const seq = React.useRef(0);

  const session = useLoad<Session>(() => api.get("/api/session"));
  const health = useLoad<Health>(() => api.get("/api/health"));
  const stats = useLoad<StatsOverview>(() => api.get("/api/stats/overview"));

  React.useEffect(() => {
    probe().then(setMode);
    return onModeChange(setMode);
  }, []);

  React.useEffect(() => {
    const stored = getTheme();
    setTheme(stored);
    document.documentElement.dataset.theme = stored;
  }, []);

  function changeTheme(next: Theme) {
    setTheme(next);
    saveTheme(next);
    document.documentElement.dataset.theme = next;
  }

  React.useEffect(() => {
    registerToast((msg, kind = "info") => {
      const id = ++seq.current;
      setToasts((t) => [...t, { id, msg, kind }]);
      setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 4200);
    });
  }, []);

  const refreshAll = React.useCallback(() => {
    session.reload(true);
    health.reload(true);
    stats.reload(true);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  async function retry() {
    const m = await probe();
    setMode(m);
    if (m === "live") refreshAll();
  }

  const active = (href: string) =>
    href === "/" ? pathname === "/" : pathname.startsWith(href);

  const toastHost = (
    <div className="toasts">
      {toasts.map((t) => (
        <div key={t.id} className="toast" data-kind={t.kind}>
          {t.kind === "ok" ? "✓" : t.kind === "err" ? "!" : "·"} {t.msg}
        </div>
      ))}
    </div>
  );

  if (mode === "unknown") return <Boot text="正在唤起本地内核…" />;
  if (mode === "offline") return <Offline onRetry={retry} />;
  if (session.loading && !session.data) return <Boot text="正在读取登录状态…" />;

  if (!session.data?.loggedIn) {
    return (
      <>
        <LoginScreen
          theme={theme}
          onThemeChange={changeTheme}
          onSuccess={() => {
            refreshAll();
          }}
        />
        {toastHost}
      </>
    );
  }

  const pending = stats.data?.pendingReview ?? 0;
  const inbox = stats.data?.inbox ?? 0;
  const user = session.data.user;
  const paperTheme = theme === "paper";

  return (
    <div className="app">
      <aside className="side">
        <Link href="/" className="side-brand">
          <span className="mark">
            <Seal size={24} />
          </span>
          <span className="name">知乎答主助手</span>
          <span className="ver">v0.1</span>
        </Link>

        <div className="side-scroll">
          {GROUPS.map((g) => (
            <div className="side-group" key={g.label}>
              <div className="side-group-label">{g.label}</div>
              {g.items.map((it) => {
                const Icon = it.icon;
                const n =
                  it.badge === "pending" ? pending : it.badge === "inbox" ? inbox : 0;
                return (
                  <Link
                    key={it.href}
                    href={it.href}
                    className="side-item"
                    data-active={active(it.href)}
                  >
                    <Icon size={16} />
                    <span>{it.label}</span>
                    {n > 0 && <span className="badge">{n}</span>}
                  </Link>
                );
              })}
            </div>
          ))}
        </div>

        <div className="side-account">
          <Link href="/settings" className="acct">
            <span className="acct-avatar">
              {user?.avatarUrl ? (
                // eslint-disable-next-line @next/next/no-img-element
                <img src={user.avatarUrl} alt="" />
              ) : (
                <IconUser size={14} />
              )}
            </span>
            <span className="acct-meta">
              <span className="acct-name">{user?.name || "已登录"}</span>
              <span className="acct-state" data-on={true}>
                <i />
                值守中 · 每日 3 次
              </span>
            </span>
          </Link>
          <button
            className="theme-toggle"
            onClick={() => changeTheme(paperTheme ? "zhihu" : "paper")}
            title={paperTheme ? "切换到知乎现代蓝" : "切换到宣纸印章黄"}
          >
            <span className="theme-swatch" data-theme={theme} />
            <span>{paperTheme ? "知乎现代蓝" : "宣纸印章黄"}</span>
            <span className="theme-toggle-arrow">切换</span>
          </button>
        </div>
      </aside>

      <div className="main">
        <div className="page-scroll">{children}</div>

        <div className="statusbar">
          <span className="sb" data-ok={health.data?.ok}>
            <i />
            内核
          </span>
          <span className="sb" data-ok={health.data?.agent}>
            <i />
            智能体
          </span>
          <span className="sep" />
          <span>{pending > 0 ? `${pending} 件待批` : "案头已清"}</span>
          <span className="sep" />
          <span>v{health.data?.version ?? "0.1.0"}</span>
          <span className="grow" />
          <span className="path" title={health.data?.dataDir}>
            {health.data?.dataDir}
          </span>
        </div>
      </div>

      {toastHost}
    </div>
  );
}
