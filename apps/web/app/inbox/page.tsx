"use client";

import React from "react";
import MemorialRow from "@/components/MemorialRow";
import ImportQuestions from "@/components/ImportQuestions";
import {
  Empty,
  IconRefresh,
  IconSearch,
  Modal,
  SectionTitle,
} from "@/components/ui";
import { api, toast } from "@/lib/api";
import { questionOpportunity } from "@/lib/creator-metrics";
import { clearDraftRecovery } from "@/lib/draft-recovery";
import { useLoad } from "@/lib/hooks";
import type { DomainCount, Job, Question, Session } from "@/lib/types";

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
  const [sort, setSort] = React.useState("fit");
  const [confirmDismiss, setConfirmDismiss] = React.useState<Question | null>(null);
  const [restoring, setRestoring] = React.useState(false);
  const [syncJob, setSyncJob] = React.useState<Job | null>(null);
  const [starting, setStarting] = React.useState(false);
  const [metricsJobId, setMetricsJobId] = React.useState<string | null>(null);
  const [metricsJob, setMetricsJob] = React.useState<Job | null>(null);

  const qs = useLoad<{ questions: Question[]; domains: DomainCount[]; metricsJobId: string | null }>(() =>
    api.get("/api/questions?status=all")
  );
  const session = useLoad<Session>(() => api.get("/api/session"));
  const canSync = session.data?.canSyncNotifications;
  const { reload } = qs;
  const all = qs.data?.questions ?? [];
  const domains = qs.data?.domains ?? [];
  const activeMetricsJob = metricsJobId ?? qs.data?.metricsJobId;
  const refreshingMetrics = Boolean(activeMetricsJob && (metricsJob?.id !== activeMetricsJob || metricsJob.state === "running"));

  const refreshMetrics = React.useCallback(async (missingOnly: boolean) => {
    try {
      const result = await api.post<{ jobId: string | null }>("/api/questions/metrics/sync", { missingOnly });
      if (result.jobId) setMetricsJobId(result.jobId);
      else if (!missingOnly) toast("收件箱没有可刷新数据的待答问题");
    } catch (error) {
      if (!missingOnly) toast(error instanceof Error ? error.message : "无法刷新机会数据", "err");
    }
  }, []);

  React.useEffect(() => { void refreshMetrics(true); }, [refreshMetrics]);

  React.useEffect(() => {
    if (!activeMetricsJob) return;
    let active = true;
    let progress = -1;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const job = await api.get<Job>(`/api/jobs/${activeMetricsJob}`);
        if (!active) return;
        setMetricsJob(job);
        if (job.state !== "running") {
          await reload(true);
          if (active) setMetricsJobId(null);
          return;
        }
        if (job.progress !== progress) {
          progress = job.progress;
          void reload(true);
        }
      } catch {
        if (!active) return;
      }
      timer = setTimeout(poll, 1000);
    }
    void poll();
    return () => { active = false; clearTimeout(timer); };
  }, [activeMetricsJob, reload]);

  React.useEffect(() => {
    if (!syncJob || syncJob.state !== "running") return;
    const timer = window.setInterval(async () => {
      try {
        const job = await api.get<Job>(`/api/jobs/${syncJob.id}`);
        setSyncJob(job);
        if (job.state === "done" || job.state === "failed") {
          window.clearInterval(timer);
          qs.reload(true);
          toast(job.message, job.state === "done" ? "ok" : "err");
        }
      } catch {
        // 单次进度查询失败时，下一次轮询继续尝试。
      }
    }, 700);
    return () => window.clearInterval(timer);
  }, [syncJob?.id, syncJob?.state]);

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
      (a, b) => Number(b.source === "imported") - Number(a.source === "imported") ||
        (a.source === "imported" && b.source === "imported" ? b.syncedAt.localeCompare(a.syncedAt) : 0) ||
        (order[a.status] ?? 3) - (order[b.status] ?? 3) ||
        (sort === "opportunity" ? (questionOpportunity(b.metrics) ?? -1) - (questionOpportunity(a.metrics) ?? -1) : 0) ||
        b.fit - a.fit
    );
  }, [all, tab, domain, kw, sort]);

  async function findQuestions() {
    setStarting(true);
    try {
      const result = await api.post<{ jobId: string }>("/api/questions/discover");
      setSyncJob({ id: result.jobId, kind: "discover_questions", questionId: null, stage: "准备", progress: 0.02,
        message: "正在准备按领域找题", state: "running", startedAt: new Date().toISOString(), finishedAt: null });
      setTab("todo");
      setDomain("all");
      setKw("");
    } catch (e) {
      toast(e instanceof Error ? e.message : "找题未能启动", "err");
    } finally {
      setStarting(false);
    }
  }

  async function doDismiss() {
    if (!confirmDismiss) return;
    try {
      const r = await api.post<{ freedBytes?: number }>(
        `/api/questions/${confirmDismiss.id}/dismiss`
      );
      clearDraftRecovery(confirmDismiss.id);
      toast(`已弃用，释放 ${Math.round((r.freedBytes ?? 0) / 1024)} KB 本地空间`, "ok");
      setConfirmDismiss(null);
      qs.reload(true);
    } catch (e) {
      toast(e instanceof Error ? e.message : "操作失败", "err");
    }
  }

  async function restore(item: Question) {
    if (restoring) return;
    setRestoring(true);
    try {
      await api.post(`/api/questions/${item.id}/restore`);
      clearDraftRecovery(item.id);
      await qs.reload(true);
      void refreshMetrics(true);
      toast("已恢复到待解答，可在「待阅」中查看", "ok");
    } catch (e) {
      toast(e instanceof Error ? e.message : "恢复失败", "err");
    } finally {
      setRestoring(false);
    }
  }

  async function syncNow(recent = false) {
    if (syncJob?.state === "running") return;
    try {
      const result = await api.post<{ jobId: string }>(recent ? "/api/questions/sync/recent" : "/api/questions/sync");
      setSyncJob({ id: result.jobId, kind: "sync", questionId: null, stage: "准备", progress: 0.02, message: recent ? "正在准备最近 3 天通知" : "正在连接知乎", state: "running", startedAt: new Date().toISOString(), finishedAt: null });
      toast(recent ? "开始同步最近 3 天的邀请与推荐" : "开始同步知乎通知", "ok");
    } catch (e) {
      toast(e instanceof Error ? e.message : "同步失败", "err");
    }
  }

  const syncing = starting || syncJob?.state === "running";

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">
            问题收件箱 <small>{all.length} QUESTIONS</small>
          </h1>
          <p className="page-sub">邀请、主动发现和手动导入的问题都在这里。手动导入置顶。</p>
        </div>
        <div className="page-actions">
          <ImportQuestions onImported={() => { setTab("todo"); setDomain("all"); setKw(""); void qs.reload(true); }} />
          <button className="btn btn-primary" onClick={() => void findQuestions()} disabled={syncing}>
            <IconSearch size={14} /> 主动找题
          </button>
          {canSync && <>
          <button className="btn" onClick={() => void syncNow(false)} disabled={syncing}>
            <IconRefresh size={14} /> 立即同步
          </button>
          <button className="btn" onClick={() => void syncNow(true)} disabled={syncing}>
            <IconRefresh size={14} /> 同步近 3 天
          </button>
          </>}
        </div>
      </header>

      {syncJob && (
        <div className="sync-progress" role="status" aria-live="polite">
          <div className="sync-progress-head"><span>{syncJob.state === "failed" ? "未完成" : syncJob.state === "done" ? "已完成" : syncJob.stage}</span><span>{Math.round(syncJob.progress * 100)}%</span></div>
          <div className="sync-progress-track"><i style={{ width: `${Math.max(2, Math.round(syncJob.progress * 100))}%` }} /></div>
          <div className="sync-progress-message">{syncJob.message}</div>
        </div>
      )}

      <div className="page-body">
      {refreshingMetrics || metricsJob ? (
        <p className="answer-sync-status" role={metricsJob?.state === "failed" && !refreshingMetrics ? "alert" : "status"}>
          {refreshingMetrics && metricsJob?.id !== activeMetricsJob ? "正在查询待答题的原题数据…" : metricsJob?.message}
        </p>
      ) : null}

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

        <div className="inbox-filters">
          <select className="select" aria-label="排序依据" value={sort} onChange={(event) => setSort(event.target.value)}>
            <option value="fit">按契合度</option>
            <option value="opportunity">按答题分</option>
          </select>
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
          <button className="btn" disabled={refreshingMetrics} onClick={() => void refreshMetrics(false)}>
            <IconRefresh size={14} /> {refreshingMetrics ? "正在补齐机会数据" : "刷新机会数据"}
          </button>
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

      <details className="creator-help">
        <summary>怎么看答题分和缺口值</summary>
        <p>答题分综合浏览、关注、回答数和问题新鲜度。浏览或关注越多、回答越少、问题越新，分数通常越高。缺少提问时间时按发布 180 天估算。</p>
        <p>缺口值是浏览数除以回答数，反映有多少浏览对应一条回答。可以结合领域契合度一起筛题。</p>
        <p>打开收件箱或同步知乎通知时，会按原题链接自动补齐缺失数据。也可以手动刷新已有数据。</p>
      </details>
      <SectionTitle count={list.length} right={<span className="dim">导入置顶 · 按状态与{sort === "opportunity" ? "答题分" : "契合度"}排序</span>}>
        清单
      </SectionTitle>

      {list.length === 0 ? (
        <Empty glyph="无">
          这个筛选下没有内容。可以换个标签、主动找题或导入知乎问题。
        </Empty>
      ) : (
        <div className="memorial-list">
          {list.map((item) => (
            <MemorialRow
              key={item.id}
              q={item}
              onDismiss={setConfirmDismiss}
              onRestore={restore}
              restoreDisabled={restoring}
              showOpportunity={!["answered", "dismissed"].includes(item.status)}
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
            之后可从「已弃用」恢复问题，但已删除的稿件与配图不会恢复。
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
