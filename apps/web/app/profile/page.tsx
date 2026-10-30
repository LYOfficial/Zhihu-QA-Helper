"use client";

import React from "react";
import {
  Empty,
  IconSpark,
  IconTrash,
  IconX,
  SectionTitle,
  Spinner,
  relTime,
} from "@/components/ui";
import { api, toast } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import type { Job, Profile, ProfileTerm } from "@/lib/types";

const GROUPS: { key: ProfileTerm["kind"]; label: string; hint: string }[] = [
  { key: "primary", label: "主领域", hint: "账号的立身之本，最多两个" },
  { key: "sub", label: "子领域", hint: "主领域下更细的分支，决定内容深度" },
  { key: "adjacent", label: "附属领域", hint: "答过且表现不差，可以迁移过去的方向" },
];

export default function ProfilePage() {
  const p = useLoad<Profile>(() => api.get("/api/profile"));
  const [terms, setTerms] = React.useState<ProfileTerm[]>([]);
  const [summary, setSummary] = React.useState("");
  const [dirty, setDirty] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [analyzing, setAnalyzing] = React.useState(false);
  const [job, setJob] = React.useState<Job | null>(null);
  const [kwEdit, setKwEdit] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (p.data) {
      setTerms(p.data.terms);
      setSummary(p.data.summary);
      setDirty(false);
    }
  }, [p.data]);

  React.useEffect(() => {
    if (!analyzing) return;
    let alive = true;
    const poll = async () => {
      try {
        const r = await api.get<{ jobs: Job[] }>("/api/jobs");
        const current = r.jobs.find((x) => x.kind === "profile");
        if (!alive || !current) return;
        setJob(current);
        if (current.state === "done" || current.state === "failed") {
          setAnalyzing(false);
          setBusy(false);
          if (current.state === "done") {
            toast("领域画像已更新", "ok");
            p.reload(true);
          } else {
            toast(`画像分析失败：${current.message}`, "err");
          }
        }
      } catch {
        // 单次轮询失败不打断后台任务
      }
    };
    poll();
    const timer = window.setInterval(poll, 900);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [analyzing, p.reload]);

  function patch(id: string, next: Partial<ProfileTerm>) {
    setTerms((t) => t.map((x) => (x.id === id ? { ...x, ...next } : x)));
    setDirty(true);
  }

  function remove(id: string) {
    setTerms((t) => t.filter((x) => x.id !== id));
    setDirty(true);
  }

  function add(kind: ProfileTerm["kind"]) {
    setTerms((t) => [
      ...t,
      {
        id: `new_${Date.now()}`,
        kind,
        label: "新词条",
        weight: 0.5,
        keywords: [],
        locked: false,
        note: "",
      },
    ]);
    setDirty(true);
  }

  async function save() {
    setBusy(true);
    try {
      await api.put("/api/profile", { summary, terms });
      toast("画像已保存", "ok");
      setDirty(false);
      p.reload(true);
    } catch (e) {
      toast(e instanceof Error ? e.message : "保存失败", "err");
    } finally {
      setBusy(false);
    }
  }

  async function analyze() {
    try {
      setBusy(true);
      setAnalyzing(true);
      setJob(null);
      await api.post("/api/profile/analyze");
      toast("已开始分析，进度会实时显示", "ok");
    } catch (e) {
      setBusy(false);
      setAnalyzing(false);
      toast(e instanceof Error ? e.message : "启动失败", "err");
    }
  }

  const status = p.data?.status ?? "empty";

  return (
    <main className="page" style={{ paddingBottom: dirty ? 96 : 72 }}>
      <header className="page-head">
        <div>
          <h1 className="page-title">
            你擅长什么 <small>CAPABILITY MAP</small>
          </h1>
          <p className="page-sub">
            {status === "ready"
              ? `基于 ${p.data?.sampleSize ?? 0} 篇历史回答与文章归纳 · ${relTime(
                  p.data?.updatedAt
                )}更新`
              : "还没有画像。登录后让大模型读一遍你的历史回答，它会替你划出能力边界。"}
          </p>
        </div>
        <div className="page-actions">
          <button className="btn" onClick={analyze} disabled={busy}>
            <IconSpark size={14} /> 重新分析
          </button>
          <button className="btn btn-primary" onClick={save} disabled={!dirty || busy}>
            {busy ? <Spinner /> : null} 保存修改
          </button>
        </div>
      </header>

      <div className="page-body">

      {(analyzing || status === "running") && (
        <div className="profile-progress">
          <div className="profile-progress-copy">
            <div className="eyebrow">LIVE PROFILE MAP</div>
            <h2>{job?.stage || "正在准备画像分析"}</h2>
            <p>{job?.message || "连接知乎，读取你的回答与文章"}</p>
            <div className="profile-progress-track">
              <i style={{ width: `${Math.max(4, (job?.progress ?? 0.04) * 100)}%` }} />
            </div>
            <span className="profile-progress-percent">
              {Math.round((job?.progress ?? 0.04) * 100)}%
            </span>
          </div>
          <div className="profile-orbit" aria-hidden="true">
            <i className="orbit-core" />
            {Array.from({ length: 7 }).map((_, i) => (
              <i key={i} className="orbit-node" style={{ "--i": i } as React.CSSProperties} />
            ))}
          </div>
        </div>
      )}

      {status === "empty" ? (
        <Empty glyph="画">
          画像还是空的。点右上角「重新分析」，我会读你最近的回答与文章，
          归纳出主领域、子领域和可迁移的附属领域。
        </Empty>
      ) : (
        <div className="split">
          <div>
            <SectionTitle right={<span className="dim">可手动修订</span>}>归纳结论</SectionTitle>
            <textarea
              className="textarea summary-quote"
              style={{
                background: "transparent",
                border: "none",
                borderLeft: "2px solid var(--rule-2)",
                resize: "vertical",
                minHeight: 120,
                paddingLeft: 16,
                outline: "none",
              }}
              value={summary}
              onChange={(e) => {
                setSummary(e.target.value);
                setDirty(true);
              }}
            />

            {(p.data?.evidence ?? []).length > 0 && (
              <div style={{ margin: "6px 0 30px" }}>
                <SectionTitle>判定依据</SectionTitle>
                {p.data!.evidence.map((e, i) => (
                  <div
                    key={i}
                    style={{
                      display: "flex",
                      gap: 10,
                      padding: "7px 0",
                      borderBottom: "1px dotted var(--rule-2)",
                      fontSize: 12,
                      color: "var(--ink-2)",
                      lineHeight: 1.7,
                    }}
                  >
                    <span className="mono" style={{ color: "var(--ink-4)" }}>
                      {i + 1}
                    </span>
                    <span>{e}</span>
                  </div>
                ))}
              </div>
            )}

            {GROUPS.map((g) => {
              const list = terms.filter((t) => t.kind === g.key);
              return (
                <div className="term-group" key={g.key}>
                  <SectionTitle
                    count={list.length}
                    right={
                      <button className="btn-ghost btn btn-sm" onClick={() => add(g.key)}>
                        + 添加
                      </button>
                    }
                  >
                    {g.label}
                  </SectionTitle>
                  <p className="hint" style={{ marginTop: -8, marginBottom: 10 }}>
                    {g.hint}
                  </p>

                  {list.length === 0 && (
                    <p className="dim" style={{ fontSize: 12, padding: "8px 0" }}>
                      暂无
                    </p>
                  )}

                  {list.map((t) => (
                    <div className="term-row" key={t.id}>
                      <div>
                        <input
                          className="term-label"
                          value={t.label}
                          onChange={(e) => patch(t.id, { label: e.target.value })}
                        />
                        {t.note && <div className="term-note">{t.note}</div>}
                        {kwEdit === t.id ? (
                          <div style={{ display: "flex", gap: 6, marginTop: 6 }}>
                            <input
                              className="input"
                              style={{ padding: "3px 8px", fontSize: 11.5 }}
                              defaultValue={t.keywords.join("、")}
                              autoFocus
                              onBlur={(e) => {
                                patch(t.id, {
                                  keywords: e.target.value
                                    .split(/[、,，\s]+/)
                                    .map((s) => s.trim())
                                    .filter(Boolean),
                                });
                                setKwEdit(null);
                              }}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                                if (e.key === "Escape") setKwEdit(null);
                              }}
                            />
                          </div>
                        ) : (
                          <div className="kw">
                            {t.keywords.slice(0, 7).map((k) => (
                              <span key={k}>{k}</span>
                            ))}
                            <button
                              onClick={() => setKwEdit(t.id)}
                              style={{
                                fontSize: 10.5,
                                color: "var(--blue)",
                                background: "transparent",
                                padding: "1px 4px",
                              }}
                            >
                              编辑关键词
                            </button>
                          </div>
                        )}
                      </div>

                      <div>
                        <input
                          className="range"
                          type="range"
                          min={0.05}
                          max={1}
                          step={0.01}
                          value={t.weight}
                          onChange={(e) => patch(t.id, { weight: Number(e.target.value) })}
                        />
                        <div className="bar" style={{ marginTop: 5 }}>
                          <i style={{ width: `${t.weight * 100}%` }} />
                        </div>
                      </div>

                      <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                        <span className="term-weight">{t.weight.toFixed(2)}</span>
                        <button
                          className="btn-icon"
                          title="删除词条"
                          onClick={() => remove(t.id)}
                        >
                          <IconTrash size={13} />
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              );
            })}
          </div>

          <aside>
            <SectionTitle>契合度怎么用</SectionTitle>
            <p style={{ fontSize: 12, color: "var(--ink-2)", lineHeight: 1.8 }}>
              每天同步到的邀请会按标题与标签去匹配这些词条的关键词，
              算出一个 0–100 的契合度。前 40% 进入重点队列自动拟稿，其余留在奏章库。
            </p>
            <div className="divider" />
            <SectionTitle>调参建议</SectionTitle>
            <div className="kv">
              <span>主领域</span>
              <b>0.9 以上</b>
            </div>
            <div className="kv">
              <span>子领域</span>
              <b>0.6 – 0.85</b>
            </div>
            <div className="kv">
              <span>附属领域</span>
              <b>0.3 – 0.6</b>
            </div>
            <p className="hint mt-8">
              权重不是越高越好。附属领域给太高，会把你根本不想答的问题塞进队列。
            </p>

            <div className="divider" />
            <SectionTitle>关键词写法</SectionTitle>
            <p className="hint">
              写同义词和别名，比如「我的世界、MC、基岩版、Java版」。
              匹配只看问题标题与标签，所以越贴近真实提问用词越准。
            </p>
          </aside>
        </div>
      )}

      {dirty && (
        <div
          style={{
            position: "fixed",
            left: "50%",
            bottom: 0,
            transform: "translateX(-50%)",
            marginLeft: "calc(var(--rail) / 2)",
            width: "min(1140px, calc(100vw - var(--rail)))",
            background: "var(--paper-raised)",
            borderTop: "1px solid var(--rule-2)",
            padding: "11px 40px",
            display: "flex",
            alignItems: "center",
            gap: 12,
            zIndex: 50,
          }}
        >
          <span style={{ fontSize: 12.5, color: "var(--ink-2)" }}>
            有未保存的修改
          </span>
          <span className="dim" style={{ fontSize: 11.5 }}>
            改动会影响之后所有问题的领域归类与契合度计算
          </span>
          <div style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
            <button
              className="btn btn-sm"
              onClick={() => {
                if (p.data) {
                  setTerms(p.data.terms);
                  setSummary(p.data.summary);
                  setDirty(false);
                }
              }}
            >
              <IconX size={12} /> 放弃
            </button>
            <button className="btn btn-sm btn-primary" onClick={save} disabled={busy}>
              保存
            </button>
          </div>
        </div>
      )}
    </div>
    </main>
  );
}
