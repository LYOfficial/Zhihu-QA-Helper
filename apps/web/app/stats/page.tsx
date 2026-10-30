"use client";

import React from "react";
import { Empty, Spinner, SectionTitle, relTime } from "@/components/ui";
import { api } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import type { DomainCount, Question, StatsOverview, StatsPoint } from "@/lib/types";

function level(v: number, max: number): 0 | 1 | 2 | 3 {
  if (v <= 0) return 0;
  const r = v / Math.max(1, max);
  if (r > 0.66) return 3;
  if (r > 0.33) return 2;
  return 1;
}

function HeatStrip({ points }: { points: StatsPoint[] }) {
  const maxC = Math.max(1, ...points.map((p) => p.collected));
  const maxA = Math.max(1, ...points.map((p) => p.answered));
  return (
    <div className="heat">
      {points.map((p, i) => (
        <div className="heat-col" key={p.date}>
          <div
            className="heat-cell"
            data-lv={level(p.collected, maxC)}
            title={`${p.date} · 收录 ${p.collected} 个问题`}
          >
            {p.collected > 0 ? p.collected : ""}
          </div>
          <div
            className="heat-cell alt"
            data-lv={level(p.answered, maxA)}
            title={`${p.date} · 完成 ${p.answered} 篇回答`}
          >
            {p.answered > 0 ? p.answered : ""}
          </div>
          <span className="heat-date">{(i === 0 || i === points.length - 1) ? p.date.slice(5) : p.date.slice(8)}</span>
        </div>
      ))}
    </div>
  );
}

export default function StatsPage() {
  const st = useLoad<StatsOverview>(() => api.get("/api/stats/overview"));
  const se = useLoad<{ series: StatsPoint[] }>(() => api.get("/api/stats/series?days=14"));
  const qs = useLoad<{ questions: Question[]; domains: DomainCount[] }>(() =>
    api.get("/api/questions?status=all")
  );

  const s = st.data;
  const points = se.data?.series ?? [];
  const domains = qs.data?.domains ?? [];
  const maxDomain = Math.max(1, ...domains.map((d) => d.count));

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">
            数据与产出 <small>LEDGER</small>
          </h1>
          <p className="page-sub">回答量、互动与本地存储。回答后的第三天起会开始出现趋势。</p>
        </div>
      </header>

      <div className="page-body">

      {!s ? (
        <div style={{ padding: 30, color: "var(--ink-3)" }}>
          <Spinner /> 正在统计…
        </div>
      ) : (
        <>
          <div className="ledger">
            <div className="ledger-cell">
              <div className="ledger-label">累计收录</div>
              <div className="ledger-value">{s.totalQuestions}</div>
              <div className="ledger-hint">含已回答与已弃用</div>
            </div>
            <div className="ledger-cell accent">
              <div className="ledger-label">待批阅</div>
              <div className="ledger-value">{s.pendingReview}</div>
              <div className="ledger-hint">素材就绪</div>
            </div>
            <div className="ledger-cell">
              <div className="ledger-label">已回答</div>
              <div className="ledger-value">{s.answered}</div>
              <div className="ledger-hint">归档率 {(s.hitRate * 100).toFixed(0)}%</div>
            </div>
            <div className="ledger-cell">
              <div className="ledger-label">获赞（30 天）</div>
              <div className="ledger-value">{s.voteups30d}</div>
              <div className="ledger-hint">新评论 {s.newComments30d}</div>
            </div>
            <div className="ledger-cell">
              <div className="ledger-label">本地占用</div>
              <div className="ledger-value">
                {s.imagesBytes >= 1024 * 1024
                  ? (s.imagesBytes / 1024 / 1024).toFixed(1)
                  : (s.imagesBytes / 1024).toFixed(0)}
                <em>{s.imagesBytes >= 1024 * 1024 ? "MB" : "KB"}</em>
              </div>
              <div className="ledger-hint">{s.images} 张配图</div>
            </div>
          </div>

          <SectionTitle right={<span className="dim">近 14 天</span>}>收录与成稿</SectionTitle>
          <div style={{ border: "1px solid var(--rule)", padding: "16px 14px 10px", background: "var(--paper-raised)" }}>
            <HeatStrip points={points} />
            <div style={{ display: "flex", gap: 18, fontSize: 11, color: "var(--ink-3)", padding: "10px 2px 2px", alignItems: "center" }}>
              <span style={{ display: "flex", alignItems: "center", gap: 5 }}>
                <i style={{ width: 9, height: 9, background: "var(--blue)", opacity: 0.75, display: "inline-block" }} />
                上行 · 收录问题
              </span>
              <span style={{ display: "flex", alignItems: "center", gap: 5 }}>
                <i style={{ width: 9, height: 9, background: "var(--cinnabar)", opacity: 0.55, display: "inline-block" }} />
                下行 · 完成回答
              </span>
            </div>
          </div>

          <div className="split mt-24">
            <div>
              <SectionTitle count={domains.length}>领域分布</SectionTitle>
              {domains.length === 0 ? (
                <Empty glyph="账">还没有数据。</Empty>
              ) : (
                domains.map((d) => (
                  <div
                    key={d.domain}
                    style={{
                      display: "grid",
                      gridTemplateColumns: "96px 1fr 40px",
                      gap: 12,
                      alignItems: "center",
                      padding: "9px 0",
                      borderBottom: "1px solid var(--rule)",
                    }}
                  >
                    <span style={{ fontFamily: "var(--font-kai)", fontSize: 14 }}>
                      {d.domain || "未分类"}
                    </span>
                    <div className="bar blue">
                      <i style={{ width: `${(d.count / maxDomain) * 100}%` }} />
                    </div>
                    <span className="mono right" style={{ fontSize: 12, color: "var(--ink-3)" }}>
                      {d.count}
                    </span>
                  </div>
                ))
              )}
            </div>

            <aside>
              <SectionTitle>产出效率</SectionTitle>
              <div className="kv">
                <span>已生成初稿</span>
                <b>{s.drafts}</b>
              </div>
              <div className="kv">
                <span>平均契合度</span>
                <b>{(s.avgFit * 100).toFixed(0)}</b>
              </div>
              <div className="kv">
                <span>估算节省</span>
                <b>{(s.savedMinutes / 60).toFixed(1)} 小时</b>
              </div>
              <div className="kv">
                <span>单稿平均</span>
                <b>{s.drafts ? Math.round(s.savedMinutes / s.drafts) : 0} 分钟</b>
              </div>

              <div className="divider" />
              <SectionTitle>说明</SectionTitle>
              <p style={{ fontSize: 11.5, color: "var(--ink-3)", lineHeight: 1.8 }}>
                「节省时间」按每篇初稿人工查阅资料约 25 分钟估算，不含你自己落笔的时间——
                那部分本来就不该被替代。
              </p>
            </aside>
          </div>
        </>
      )}
    </div>
    </main>
  );
}
