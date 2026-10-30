"use client";

import React from "react";
import MemorialRow from "@/components/MemorialRow";
import {
  Empty,
  IconRefresh,
  IconSearch,
  Modal,
  SectionTitle,
} from "@/components/ui";
import { api, toast } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import { watchJob } from "@/lib/jobwatch";
import type { DomainCount, Question } from "@/lib/types";

const TABS = [
  { key: "todo", label: "全部待办" },
  { key: "pending_review", label: "待批" },
  { key: "inbox", label: "待阅" },
  { key: "researching", label: "拟稿中" },
  { key: "answered", label: "已回答" },
  { key: "dismissed", label: "已弃用" },
];

export default function InboxPage() {
  const [tab, setTab] = React.useState("todo");
  const [domain, setDomain] = React.useState("all");
  const [kw, setKw] = React.useState("");
  const [confirmDismiss, setConfirmDismiss] = React.useState<Question | null>(null);

  const qs = useLoad<{ questions: Question[]; domains: DomainCount[] }>(() =>
    api.get("/api/questions?status=all")
  );

  const all = qs.data?.questions ?? [];
  const domains = qs.data?.domains ?? [];

  const list = React.useMemo(() => {
    let l = [...all];
    if (tab === "todo") {
      l = l.filter((x) => ["inbox", "researching", "pending_review"].includes(x.status));
    } else {
      l = l.filter((x) => x.status === tab);
    }
    if (domain !== "all") l = l.filter((x) => x.domain === domain);
    if (kw.trim()) {
      const k = kw.trim().toLowerCase();
      l = l.filter((x) => (x.title + x.excerpt + x.tags.join(" ")).toLowerCase().includes(k));
    }
    const order: Record<string, number> = { pending_review: 0, researching: 1, inbox: 2 };
    return l.sort(
      (a, b) => (order[a.status] ?? 3) - (order[b.status] ?? 3) || b.fit - a.fit
    );
  }, [all, tab, domain, kw]);

  async function research(item: Question) {
    try {
      await api.post(`/api/questions/${item.id}/research`);
      toast("已交给智能体，正在检索资料并起草", "ok");
      setTimeout(() => qs.reload(true), 1200);
      watchJob(item.id, "research", () => qs.reload(true));
    } catch (e) {
      toast(e instanceof Error ? e.message : "启动失败", "err");
    }
  }

  async function doDismiss() {
    if (!confirmDismiss) return;
    try {
      const r = await api.post<{ freedBytes?: number }>(
        `/api/questions/${confirmDismiss.id}/dismiss`
      );
      toast(`已弃用，释放 ${Math.round((r.freedBytes ?? 0) / 1024)} KB 本地空间`, "ok");
      setConfirmDismiss(null);
      qs.reload(true);
    } catch (e) {
      toast(e instanceof Error ? e.message : "操作失败", "err");
    }
  }

  async function syncNow() {
    try {
      await api.post("/api/questions/sync");
      toast("开始同步知乎通知", "ok");
      setTimeout(() => qs.reload(true), 2800);
    } catch (e) {
      toast(e instanceof Error ? e.message : "同步失败", "err");
    }
  }

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">
            全部奏章 <small>{all.length} MEMORIALS</small>
          </h1>
          <p className="page-sub">
            自动登录后按领域归档；重点队列取契合度最高的前 40%，其余留在这里等你偶遇。
          </p>
        </div>
        <div className="page-actions">
          <button className="btn" onClick={syncNow}>
            <IconRefresh size={14} /> 立即同步
          </button>
        </div>
      </header>

      <div className="page-body">

      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 14,
          flexWrap: "wrap",
          marginBottom: 16,
        }}
      >
        <div style={{ display: "flex", gap: 2, flexWrap: "wrap" }}>
          {TABS.map((t) => {
            const n =
              t.key === "todo"
                ? all.filter((x) => ["inbox", "researching", "pending_review"].includes(x.status))
                    .length
                : all.filter((x) => x.status === t.key).length;
            return (
              <button
                key={t.key}
                className="btn btn-sm"
                onClick={() => setTab(t.key)}
                style={{
                  borderColor: tab === t.key ? "var(--ink-3)" : "transparent",
                  background: tab === t.key ? "var(--paper-raised)" : "transparent",
                  color: tab === t.key ? "var(--ink)" : "var(--ink-3)",
                }}
              >
                {t.label}
                <span className="mono" style={{ color: "var(--ink-4)" }}>
                  {n}
                </span>
              </button>
            );
          })}
        </div>

        <div style={{ marginLeft: "auto", display: "flex", gap: 8, alignItems: "center" }}>
          <div style={{ position: "relative" }}>
            <span
              style={{
                position: "absolute",
                left: 8,
                top: 7,
                color: "var(--ink-4)",
                pointerEvents: "none",
              }}
            >
              <IconSearch size={14} />
            </span>
            <input
              className="input"
              style={{ width: 210, paddingLeft: 28, paddingTop: 6, paddingBottom: 6 }}
              placeholder="搜标题、标签"
              value={kw}
              onChange={(e) => setKw(e.target.value)}
            />
          </div>
        </div>
      </div>

      {domains.length > 0 && (
        <div style={{ display: "flex", gap: 7, flexWrap: "wrap", marginBottom: 20 }}>
          <button
            className={`chip${domain === "all" ? " blue" : ""}`}
            onClick={() => setDomain("all")}
            style={{ cursor: "pointer" }}
          >
            全部领域
          </button>
          {domains.map((d) => (
            <button
              key={d.domain}
              className={`chip${domain === d.domain ? " blue" : ""}`}
              onClick={() => setDomain(d.domain)}
              style={{ cursor: "pointer" }}
            >
              {d.domain || "未分类"}
              <span className="mono" style={{ opacity: 0.55 }}>
                {d.count}
              </span>
            </button>
          ))}
        </div>
      )}

      <SectionTitle count={list.length} right={<span className="dim">契合度降序</span>}>
        清单
      </SectionTitle>

      {list.length === 0 ? (
        <Empty glyph="无">
          这个筛选下没有内容。换个标签，或者点「立即同步」去看看知乎上有没有新的邀请。
        </Empty>
      ) : (
        <div className="memorial-list">
          {list.map((item) => (
            <MemorialRow
              key={item.id}
              q={item}
              onResearch={research}
              onDismiss={setConfirmDismiss}
            />
          ))}
        </div>
      )}

      {confirmDismiss && (
        <Modal
          title="弃用这件奏章？"
          sub={`「${confirmDismiss.title}」`}
          onClose={() => setConfirmDismiss(null)}
        >
          <p style={{ fontSize: 12.5, color: "var(--ink-2)", lineHeight: 1.8 }}>
            会删除本地已生成的正稿与配图，释放磁盘空间，并把这个问题标记为「弃用」。
            知乎上的问题本身不受影响，之后仍可以手动找回。
          </p>
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 20 }}>
            <button className="btn" onClick={() => setConfirmDismiss(null)}>
              再想想
            </button>
            <button className="btn btn-red" onClick={doDismiss}>
              确认弃用
            </button>
          </div>
        </Modal>
      )}
    </div>
    </main>
  );
}
