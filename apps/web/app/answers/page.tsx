"use client";

import React from "react";
import { Empty, IconExternal, IconRefresh, IconSearch, Modal, Spinner } from "@/components/ui";
import { api, openExternal, toast } from "@/lib/api";
import { answerQuality, formatMetric, qualityLevel } from "@/lib/creator-metrics";
import { useLoad } from "@/lib/hooks";
import type { AnswerMetrics, CreatorAnswer, Job, Session } from "@/lib/types";

interface AnswerPageData {
  answers: CreatorAnswer[];
  total: number;
  pageSize: number;
  syncedAt: string | null;
  jobId: string | null;
}

const dateTime = new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium", timeStyle: "short" });
const safeTags = new Set([
  "p", "br", "hr", "b", "strong", "i", "em", "s", "del", "u", "sub", "sup", "blockquote",
  "pre", "code", "ul", "ol", "li", "div", "h1", "h2", "h3", "h4", "figure", "figcaption",
  "table", "thead", "tbody", "tr", "th", "td",
]);

function safeUrl(raw: string | null) {
  if (!raw) return null;
  try {
    const url = new URL(raw, "https://www.zhihu.com/");
    return ["https:", "http:"].includes(url.protocol) ? url : null;
  } catch {
    return null;
  }
}

function renderAnswerNode(node: Node, key: number): React.ReactNode {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (!(node instanceof HTMLElement)) return null;
  const tag = node.localName;
  if (["script", "style", "iframe", "template", "object"].includes(tag)) return null;
  if (tag === "img") {
    const url = safeUrl(node.getAttribute("data-original") || node.getAttribute("src") || node.getAttribute("data-src"));
    if (url?.protocol === "https:" && (url.hostname === "zhimg.com" || url.hostname.endsWith(".zhimg.com"))) {
      return <img key={key} src={url.href} alt={node.getAttribute("alt") || ""} loading="lazy" referrerPolicy="no-referrer" />;
    }
    return null;
  }
  const children = Array.from(node.childNodes, renderAnswerNode);
  if (tag === "a") {
    const url = safeUrl(node.getAttribute("href"));
    if (url) return <button key={key} className="answer-link" onClick={() => void openExternal(url.href)}>{children}</button>;
  }
  if (tag === "br" || tag === "hr") return React.createElement(tag, { key });
  return React.createElement(safeTags.has(tag) ? tag : "span", { key }, children);
}

function AnswerBody({ html }: { html: string }) {
  const nodes = React.useMemo(() => {
    const template = document.createElement("template");
    template.innerHTML = html;
    return Array.from(template.content.childNodes, renderAnswerNode);
  }, [html]);
  return <div className="answer-content">{nodes}</div>;
}

function AnswerStats({ metrics }: { metrics: AnswerMetrics }) {
  const quality = answerQuality(metrics);
  const level = quality === null ? null : qualityLevel(quality);
  const percent = (count: number | null) => !metrics.reads || count === null
    ? "—"
    : `${(100 * count / metrics.reads).toFixed(2)}%`;
  const counters = [
    ["阅读", metrics.reads], ["赞同", metrics.votes], ["评论", metrics.comments],
    ["收藏", metrics.favorites], ["喜欢", metrics.likes],
  ] as const;
  return (
    <div className="answer-stats">
      <dl className="answer-counts">
        {counters.map(([label, value]) => (
          <div key={label}><dt>{label}</dt><dd>{formatMetric(value)}</dd></div>
        ))}
      </dl>
      <div className="creator-metrics">
        <span className="creator-grade" data-tone={level?.tone ?? "low"}>
          {level ? `质效分 ${formatMetric(quality)} · ${level.label}` : "互动数据待补齐"}
        </span>
        <span>赞同率 <b>{percent(metrics.votes)}</b></span>
        <span>收藏率 <b>{percent(metrics.favorites)}</b></span>
      </div>
    </div>
  );
}

function AnswerDetail({ answer, onClose }: { answer: CreatorAnswer; onClose: () => void }) {
  const body = useLoad<{ content: string; truncated: boolean }>(() => api.get(`/api/answers/${answer.id}`));
  return (
    <Modal title={answer.title} sub={answer.createdTime ? dateTime.format(answer.createdTime * 1000) : undefined} width={820} onClose={onClose}>
      <AnswerStats metrics={answer.metrics} />
      <div className="answer-detail-copy" aria-busy={body.loading}>
        {body.loading ? <p><Spinner /> 正在读取回答正文</p> : body.data ? (
          <>
            {body.data.truncated && <p className="dim">知乎只提供了部分正文，可在原回答中继续阅读。</p>}
            <AnswerBody html={body.data.content} />
          </>
        ) : (
          <>
            <p role="alert">{body.error || "正文暂不可用"}</p>
            {answer.excerpt && <p className="dim">{answer.excerpt}</p>}
            <button className="btn btn-sm" onClick={() => void body.reload()}>重试读取</button>
          </>
        )}
      </div>
      <div className="answer-detail-actions">
        <button className="btn" onClick={() => void openExternal(`https://www.zhihu.com/question/${answer.questionId}/answer/${answer.id}`)}>
          <IconExternal size={14} /> 在知乎打开
        </button>
      </div>
    </Modal>
  );
}

export default function AnswersPage() {
  const [input, setInput] = React.useState("");
  const [keyword, setKeyword] = React.useState("");
  const [page, setPage] = React.useState(0);
  const [selected, setSelected] = React.useState<CreatorAnswer | null>(null);
  const [starting, setStarting] = React.useState(false);
  const [jobId, setJobId] = React.useState<string | null>(null);
  const [job, setJob] = React.useState<Job | null>(null);
  const list = useLoad<AnswerPageData>(() => {
    const params = new URLSearchParams({ offset: String(page * 20), q: keyword });
    return api.get(`/api/answers?${params}`);
  }, [page, keyword]);
  const session = useLoad<Session>(() => api.get("/api/session"));
  const canSync = session.data?.canSyncNotifications;
  const { reload } = list;
  const activeJob = jobId ?? list.data?.jobId;
  const busy = starting || Boolean(activeJob && (!job || job.id !== activeJob || job.state === "running"));

  React.useEffect(() => {
    if (!activeJob) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const next = await api.get<Job>(`/api/jobs/${activeJob}`);
        if (!alive) return;
        setJob(next);
        if (next.state !== "running") {
          setJobId(null);
          void reload(true);
          return;
        }
      } catch {
        if (!alive) return;
      }
      timer = setTimeout(poll, 1000);
    }
    void poll();
    return () => { alive = false; clearTimeout(timer); };
  }, [activeJob, reload]);

  async function sync() {
    setStarting(true);
    setJob(null);
    try {
      const next = await api.post<{ jobId: string }>("/api/answers/sync");
      setJobId(next.jobId);
    } catch (error) {
      toast(error instanceof Error ? error.message : "无法开始同步", "err");
    } finally {
      setStarting(false);
    }
  }

  const pages = list.data ? Math.max(1, Math.ceil(list.data.total / list.data.pageSize)) : 1;
  React.useEffect(() => {
    if (list.data && !list.loading && page >= pages) setPage(pages - 1);
  }, [list.data, list.loading, page, pages]);

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">历史作答</h1>
          <p className="page-sub">{list.data?.syncedAt ? `上次同步 ${dateTime.format(new Date(list.data.syncedAt))}` : canSync ? "从知乎创作中心同步本人作答和互动数据" : "查看此账号已保存的历史作答"}</p>
        </div>
        {canSync && <div className="page-actions">
          <button className="btn btn-primary" onClick={() => void sync()} disabled={busy}>
            <IconRefresh size={14} /> {busy ? "同步中" : "同步作答"}
          </button>
        </div>}
      </header>
      <div className="page-body">
        {job && <p className="answer-sync-status" role={job.state === "failed" ? "alert" : "status"}>{job.message}</p>}
        <div className="answer-toolbar">
          <form onSubmit={(event) => { event.preventDefault(); setKeyword(input.trim()); setPage(0); }}>
            <input className="input" aria-label="搜索作答" placeholder="搜问题标题或回答摘要" value={input} onChange={(event) => setInput(event.target.value)} />
            <button className="btn" type="submit"><IconSearch size={14} /> 搜索</button>
            {keyword && <button className="btn" type="button" onClick={() => { setInput(""); setKeyword(""); setPage(0); }}>清除</button>}
          </form>
          <span className="dim">{list.data ? `${list.data.total} 条作答` : ""}</span>
        </div>
        <details className="creator-help">
          <summary>质效分怎么算</summary>
          <p>加权互动 = 赞同 + 2 × 评论 + 1.5 × 收藏 + 0.5 × 喜欢。质效分 = 1,000 × 加权互动 ÷（阅读数 + 1,000）。</p>
          <p>80 分起为极优，40 分起为高质，15 分起为中等。较少的阅读样本需要继续观察。</p>
        </details>
        {list.error && <p role="alert">{list.error} <button className="btn btn-sm" onClick={() => void reload()}>重试</button></p>}
        {list.loading ? <p role="status"><Spinner /> 正在读取本机作答记录</p> : list.data?.answers.length ? (
          <div className="answer-list">
            {list.data.answers.map((answer) => (
              <article key={answer.id} className="answer-card">
                {answer.createdTime && <time className="dim" dateTime={new Date(answer.createdTime * 1000).toISOString()}>{dateTime.format(answer.createdTime * 1000)}</time>}
                <button className="answer-title" onClick={() => setSelected(answer)}>{answer.title}</button>
                {answer.excerpt && <p className="answer-excerpt">{answer.excerpt}</p>}
                <AnswerStats metrics={answer.metrics} />
              </article>
            ))}
          </div>
        ) : !list.error && (
          <Empty glyph="答">{keyword ? "没有找到匹配的作答。" : list.data?.syncedAt ? "知乎创作中心没有返回作答记录。" : canSync ? "点击「同步作答」，在这里查看历史回答。" : "此账号暂无已保存的回答。知乎授权暂不支持创作中心同步。"}</Empty>
        )}
        {pages > 1 && (
          <nav aria-label="作答分页" className="answer-pagination">
            <button className="btn btn-sm" disabled={page === 0 || list.loading} onClick={() => setPage(page - 1)}>上一页</button>
            <span>{page + 1} / {pages} 页</span>
            <button className="btn btn-sm" disabled={page + 1 >= pages || list.loading} onClick={() => setPage(page + 1)}>下一页</button>
          </nav>
        )}
      </div>
      {selected && <AnswerDetail key={selected.id} answer={selected} onClose={() => setSelected(null)} />}
    </main>
  );
}
