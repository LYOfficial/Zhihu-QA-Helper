"use client";

import { useRouter } from "next/navigation";
import React from "react";
import MemorialRow from "@/components/MemorialRow";
import {
  Empty,
  IconArrowRight,
  IconRefresh,
  IconSpark,
  SectionTitle,
  today,
} from "@/components/ui";
import { api, toast } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import { watchJob } from "@/lib/jobwatch";
import type {
  DomainCount,
  EventItem,
  Question,
  Session,
  Settings,
  StatsOverview,
} from "@/lib/types";

function slotStates(times: string[], lastSyncAt: string | null) {
  const now = new Date();
  const sync = lastSyncAt ? new Date(lastSyncAt) : null;
  const rows = times.map((t) => {
    const [h, m] = t.split(":").map(Number);
    const slot = new Date(now);
    slot.setHours(h || 0, m || 0, 0, 0);
    const done = !!sync && sync.getTime() >= slot.getTime();
    return { time: t, done, past: now.getTime() > slot.getTime() };
  });
  const nextIdx = rows.findIndex((r) => !r.done && !r.past);
  return rows.map((r, i) => ({
    ...r,
    state: r.done ? "done" : i === nextIdx ? "live" : "",
  }));
}

export default function DeskHome() {
  const router = useRouter();
  const q = useLoad<{ questions: Question[]; domains: DomainCount[] }>(() =>
    api.get("/api/questions")
  );
  const ev = useLoad<{ events: EventItem[] }>(() => api.get("/api/events"));
  const st = useLoad<StatsOverview>(() => api.get("/api/stats/overview"));
  const se = useLoad<Settings>(() => api.get("/api/settings"));
  const ss = useLoad<Session>(() => api.get("/api/session"));

  const all = q.data?.questions ?? [];
  const pending = all.filter((x) => x.status === "pending_review");
  const researching = all.filter((x) => x.status === "researching");
  const queue = all.filter((x) => x.status === "inbox" && x.bucket === "selected");

  const slots = slotStates(se.data?.cronTimes ?? ["07:00", "15:00", "20:00"], ss.data?.lastSyncAt ?? null);

  async function syncNow() {
    try {
      await api.post("/api/questions/sync");
      toast("已开始同步，稍等片刻看结果", "ok");
      setTimeout(() => {
        q.reload(true);
        ev.reload(true);
        st.reload(true);
      }, 2600);
    } catch (e) {
      toast(e instanceof Error ? e.message : "同步失败", "err");
    }
  }

  async function research(item: Question) {
    try {
      await api.post(`/api/questions/${item.id}/research`);
      toast("智能体已接手，正在检索资料", "ok");
      q.reload(true);
      watchJob(item.id, "research", () => {
        q.reload(true);
        st.reload(true);
        ev.reload(true);
      });
    } catch (e) {
      toast(e instanceof Error ? e.message : "启动失败", "err");
    }
  }

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">
            {pending.length > 0 ? `今日待批 ${pending.length} 件` : "今日无待批"}
            <small>ON THE DESK</small>
          </h1>
          <p className="page-sub">
            {ss.data?.user
              ? `${ss.data.user.name} · ${ss.data.user.headline || "知乎答主"}`
              : "尚未登录知乎账号"}
            {researching.length > 0 && ` · ${researching.length} 件智能体正在拟稿`}
          </p>
        </div>
        <div className="page-actions">
          <button className="btn" onClick={syncNow}>
            <IconRefresh size={14} /> 同步通知
          </button>
          <button className="btn btn-primary" onClick={() => router.push("/desk")}>
            开始批阅 <IconArrowRight size={14} />
          </button>
        </div>
      </header>

      <div className="page-body">

      <div className="ledger">
        <div className="ledger-cell accent">
          <div className="ledger-label">待人工批阅</div>
          <div className="ledger-value">{st.data?.pendingReview ?? pending.length}</div>
          <div className="ledger-hint">素材已备齐，等你落笔</div>
        </div>
        <div className="ledger-cell">
          <div className="ledger-label">待阅奏章</div>
          <div className="ledger-value">{st.data?.inbox ?? 0}</div>
          <div className="ledger-hint">未进入重点队列</div>
        </div>
        <div className="ledger-cell">
          <div className="ledger-label">累计已回答</div>
          <div className="ledger-value">
            {st.data?.answered ?? 0}
            <em>件</em>
          </div>
          <div className="ledger-hint">近 30 天 {st.data?.answers30d ?? 0} 件</div>
        </div>
        <div className="ledger-cell">
          <div className="ledger-label">平均契合度</div>
          <div className="ledger-value">{((st.data?.avgFit ?? 0) * 100).toFixed(0)}</div>
          <div className="ledger-hint">低于 60 建议直接弃用</div>
        </div>
        <div className="ledger-cell">
          <div className="ledger-label">估算省下</div>
          <div className="ledger-value">
            {((st.data?.savedMinutes ?? 0) / 60).toFixed(1)}
            <em>小时</em>
          </div>
          <div className="ledger-hint">按每稿 25 分钟计</div>
        </div>
      </div>

      <div className="split">
        <div>
          <SectionTitle
            count={pending.length}
            right={
              <button className="btn-ghost btn btn-sm" onClick={() => router.push("/desk")}>
                全部批阅 →
              </button>
            }
          >
            待批奏章
          </SectionTitle>

          {pending.length === 0 && (
            <Empty glyph="空">
              目前没有待批阅的稿子。可以在「奏章」里挑一个感兴趣的问题，
              点播放键让智能体开始拟稿。
            </Empty>
          )}

          <div className="memorial-list" style={{ borderTop: pending.length ? undefined : "none" }}>
            {pending.slice(0, 6).map((item) => (
              <MemorialRow key={item.id} q={item} onResearch={research} />
            ))}
          </div>

          {queue.length > 0 && (
            <>
              <div className="mt-24">
                <SectionTitle
                  count={queue.length}
                  right={<span className="dim">按契合度排序 · 未启动</span>}
                >
                  重点队列
                </SectionTitle>
              </div>
              <div className="memorial-list">
                {queue.slice(0, 5).map((item) => (
                  <MemorialRow key={item.id} q={item} onResearch={research} compact />
                ))}
              </div>
            </>
          )}
        </div>

        <aside>
          <SectionTitle right="每日 3 次">值守时刻</SectionTitle>
          <div className="timeline">
            {slots.map((s) => (
              <div key={s.time} className="tl-item" data-state={s.state}>
                <div className="tl-time">{s.time}</div>
                <div className="tl-title">
                  {s.state === "done" ? "已同步" : s.state === "live" ? "下一次同步" : "未执行"}
                </div>
                <div className="tl-desc">
                  {s.state === "done"
                    ? "抓取邀请与推荐，归入领域文件夹"
                    : s.state === "live"
                      ? "到点自动登录并拉取通知"
                      : "本次未触发（软件可能未运行）"}
                </div>
              </div>
            ))}
          </div>

          <div className="divider" />

          <SectionTitle count={ev.data?.events.length} right={<span className="dim">最近</span>}>
            近期动态
          </SectionTitle>
          <div>
            {(ev.data?.events ?? []).slice(0, 7).map((e) => (
              <div
                key={e.id}
                style={{
                  padding: "9px 0",
                  borderBottom: "1px dotted var(--rule-2)",
                  display: "flex",
                  gap: 10,
                }}
              >
                <span
                  style={{
                    width: 3,
                    flexShrink: 0,
                    background:
                      e.kind === "verify" || e.kind === "research"
                        ? "var(--cinnabar)"
                        : "var(--rule-2)",
                    opacity: 0.7,
                  }}
                />
                <div style={{ minWidth: 0 }}>
                  <div style={{ fontSize: 12.5, color: "var(--ink)" }}>{e.title}</div>
                  <div
                    style={{
                      fontSize: 11,
                      color: "var(--ink-3)",
                      lineHeight: 1.6,
                      marginTop: 2,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      display: "-webkit-box",
                      WebkitLineClamp: 2,
                      WebkitBoxOrient: "vertical",
                    }}
                  >
                    {e.detail}
                  </div>
                </div>
              </div>
            ))}
            {(ev.data?.events ?? []).length === 0 && (
              <p className="dim" style={{ fontSize: 12, padding: "10px 0" }}>
                还没有动态。完成一次同步或拟稿后，这里会留下记录。
              </p>
            )}
          </div>

          <div className="divider" />

          <SectionTitle count={q.data?.domains.length}>领域分布</SectionTitle>
          <div style={{ marginBottom: 4 }}>
            {(q.data?.domains ?? []).slice(0, 5).map((d) => {
              const maxD = Math.max(1, ...(q.data?.domains ?? []).map((x) => x.count));
              return (
                <div
                  key={d.domain}
                  style={{
                    display: "grid",
                    gridTemplateColumns: "82px 1fr 22px",
                    gap: 8,
                    alignItems: "center",
                    padding: "5px 0",
                  }}
                >
                  <span style={{ fontSize: 11.5, color: "var(--ink-2)" }}>
                    {d.domain || "未分类"}
                  </span>
                  <div className="bar blue">
                    <i style={{ width: `${(d.count / maxD) * 100}%` }} />
                  </div>
                  <span className="mono right" style={{ fontSize: 11, color: "var(--ink-4)" }}>
                    {d.count}
                  </span>
                </div>
              );
            })}
          </div>

          <div className="divider" />

          <div
            style={{
              border: "1px solid var(--rule)",
              padding: "14px 15px",
              background: "linear-gradient(160deg, var(--paper-raised), transparent)",
            }}
          >
            <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 6 }}>
              <IconSpark size={14} />
              <span style={{ fontSize: 12, letterSpacing: "0.08em" }}>本机规矩</span>
            </div>
            <p style={{ fontSize: 11.5, color: "var(--ink-3)", lineHeight: 1.75 }}>
              AI 只负责把材料摆到案头，落笔永远是你。每篇稿子都保留核查记录与来源，
              引用可以追到原文。
            </p>
          </div>
        </aside>
      </div>
    </div>
    </main>
  );
}
