"use client";

import { useRouter, useSearchParams } from "next/navigation";
import React, { Suspense } from "react";
import {
  Empty,
  IconCheck,
  IconCopy,
  IconExternal,
  IconImage,
  IconQuill,
  IconRefresh,
  IconX,
  Modal,
  SectionTitle,
  Spinner,
} from "@/components/ui";
import { api, imageSrc, openExternal, toast } from "@/lib/api";
import { useCopy, useLoad } from "@/lib/hooks";
import type { Draft, Job, Question, Settings } from "@/lib/types";

/* ------------------------------------------------------------ markdown 渲染 */

function inline(text: string, key: string) {
  const parts = text.split(/(\*\*[^*]+\*\*)/g);
  return parts.map((p, i) =>
    p.startsWith("**") && p.endsWith("**") ? (
      <strong key={`${key}-${i}`} style={{ fontWeight: 600 }}>
        {p.slice(2, -2)}
      </strong>
    ) : (
      <React.Fragment key={`${key}-${i}`}>{p}</React.Fragment>
    )
  );
}

function citationHost(url: string) {
  try {
    return new URL(url).hostname;
  } catch {
    return "来源链接";
  }
}

function RenderMarkdown({
  md,
  qid,
  images,
  onPick,
}: {
  md: string;
  qid: string;
  images: Draft["images"];
  /** 预览态点击某一段时，把「插入点」落到这一段结尾，供插图使用 */
  onPick?: (offset: number) => void;
}) {
  const lines = md.split("\n");
  const blocks: React.ReactNode[] = [];
  let list: string[] = [];
  // 每一行在全文中的起始偏移，用于把预览里的点击换算成正文插入位置
  const starts: number[] = [];
  let cursor = 0;
  for (const line of lines) {
    starts.push(cursor);
    cursor += line.length + 1;
  }
  const after = (idx: number) => Math.min(md.length, starts[idx] + lines[idx].length);

  const flushList = (k: number) => {
    if (!list.length) return;
    blocks.push(
      <ul key={`ul-${k}`} data-off={after(k - 1)} style={{ margin: "0 0 1em", paddingLeft: 20 }}>
        {list.map((li, i) => (
          <li key={i} style={{ marginBottom: 4 }}>
            {inline(li, `li-${k}-${i}`)}
          </li>
        ))}
      </ul>
    );
    list = [];
  };

  lines.forEach((raw, idx) => {
    const line = raw.trimEnd();
    const img = line.match(/^\[IMG:([^\]]+)\]$/);
    if (img) {
      flushList(idx);
      const file = img[1];
      const asset = images.find((x) => x.filename === file);
      blocks.push(
        <figure
          key={`fig-${idx}`}
          data-off={after(idx)}
          style={{ margin: "18px 0", textAlign: "center" }}
        >
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src={imageSrc(qid, file)}
            alt={asset?.caption || file}
            style={{
              maxWidth: "min(100%, 440px)",
              width: "100%",
              border: "1px solid var(--rule)",
              background: "var(--paper)",
            }}
          />
          <figcaption
            style={{
              fontSize: 11.5,
              color: "var(--ink-3)",
              marginTop: 6,
              fontFamily: "var(--font-sans)",
            }}
          >
            {asset?.caption || file}
            {asset?.sourceName ? ` · ${asset.sourceName}` : ""}
          </figcaption>
        </figure>
      );
      return;
    }
    if (/^###\s/.test(line)) {
      flushList(idx);
      blocks.push(
        <h4
          key={idx}
          data-off={after(idx)}
          style={{
            fontFamily: "var(--font-kai)",
            fontSize: 15.5,
            fontWeight: 400,
            margin: "1.1em 0 .4em",
          }}
        >
          {line.replace(/^###\s/, "")}
        </h4>
      );
      return;
    }
    if (/^##\s/.test(line)) {
      flushList(idx);
      blocks.push(
        <h3
          key={idx}
          data-off={after(idx)}
          style={{
            fontFamily: "var(--font-kai)",
            fontSize: 19,
            fontWeight: 400,
            margin: "1.4em 0 .5em",
            letterSpacing: "0.02em",
          }}
        >
          {line.replace(/^##\s/, "")}
        </h3>
      );
      return;
    }
    if (/^[-*]\s/.test(line)) {
      list.push(line.replace(/^[-*]\s/, ""));
      return;
    }
    if (/^>\s?/.test(line)) {
      flushList(idx);
      blocks.push(
        <blockquote
          key={idx}
          data-off={after(idx)}
          style={{
            margin: "0 0 1em",
            paddingLeft: 14,
            borderLeft: "2px solid var(--rule-2)",
            color: "var(--ink-2)",
            fontFamily: "var(--font-kai)",
          }}
        >
          {inline(line.replace(/^>\s?/, ""), `bq-${idx}`)}
        </blockquote>
      );
      return;
    }
    if (!line.trim()) {
      flushList(idx);
      return;
    }
    flushList(idx);
    blocks.push(
      <p key={idx} data-off={after(idx)} style={{ margin: "0 0 1.05em", lineHeight: 2 }}>
        {inline(line, `p-${idx}`)}
      </p>
    );
  });
  flushList(9999);

  return (
    <div
      style={{ fontSize: 15, lineHeight: 2, color: "var(--ink)" }}
      onClick={(e) => {
        if (!onPick) return;
        const el = (e.target as HTMLElement).closest("[data-off]");
        if (!el) return;
        const off = Number(el.getAttribute("data-off"));
        if (Number.isFinite(off)) onPick(off);
      }}
    >
      {blocks}
    </div>
  );
}

/* ------------------------------------------------------------ 主组件 */

function DeskInner() {
  const sp = useSearchParams();
  const router = useRouter();
  const wantId = sp.get("id") || "";

  const qs = useLoad<{ questions: Question[] }>(() => api.get("/api/questions"));
  const se = useLoad<Settings>(() => api.get("/api/settings"));
  const pending = (qs.data?.questions ?? []).filter((x) =>
    ["pending_review", "researching", "inbox"].includes(x.status)
  );

  const current =
    pending.find((x) => x.id === wantId) ||
    pending.find((x) => x.status === "pending_review") ||
    pending[0];

  const [md, setMd] = React.useState("");
  const [images, setImages] = React.useState<Draft["images"]>([]);
  const [doubts, setDoubts] = React.useState<Draft["doubts"]>([]);
  const [citations, setCitations] = React.useState<Draft["citations"]>([]);
  const [warnings, setWarnings] = React.useState<string[]>([]);
  const [liveJob, setLiveJob] = React.useState<{
    stage: string;
    message: string;
    state: string;
    elapsed: number;
  } | null>(null);
  const [mode, setMode] = React.useState<"edit" | "preview">("preview");
  const [savedAt, setSavedAt] = React.useState<string>("");
  const [loading, setLoading] = React.useState(false);
  const [confirm, setConfirm] = React.useState<null | "answered" | "dismiss">(null);
  const [tab, setTab] = React.useState<"doubt" | "source">("doubt");
  const taRef = React.useRef<HTMLTextAreaElement>(null);
  const dirty = React.useRef(false);
  // 记住最后一次落下的插入点：编辑态是选区，预览态是点中的段落结尾。
  // 之前预览态直接 append 到 md 末尾，插图永远跑到文末，这就是那个 bug。
  const caretRef = React.useRef<number | null>(null);
  const pendingCaret = React.useRef<number | null>(null);
  const pendingScroll = React.useRef<number | null>(null);
  const { copy } = useCopy();

  const draftId = current?.id ?? "";

  React.useEffect(() => {
    if (!draftId) return;
    let alive = true;
    setLoading(true);
    api
      .get<Draft>(`/api/drafts/${draftId}`)
      .then((d) => {
        if (!alive) return;
        setMd(d.markdown || "");
        setImages(d.images || []);
        setDoubts(d.doubts || []);
        setCitations(d.citations || []);
        setWarnings(d.warnings || []);
        setSavedAt(d.updatedAt ? "已加载" : "");
        dirty.current = false;
        setMode(d.markdown?.trim() ? "preview" : "edit");
      })
      .catch((e) => toast(e instanceof Error ? e.message : "读取失败", "err"))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [draftId]);

  // 自动保存
  React.useEffect(() => {
    if (!draftId || !dirty.current) return;
    const t = setTimeout(async () => {
      try {
        await api.put(`/api/drafts/${draftId}`, { markdown: md });
        setSavedAt(
          `已保存 ${new Date().toLocaleTimeString("zh-CN", {
            hour: "2-digit",
            minute: "2-digit",
          })}`
        );
      } catch {
        setSavedAt("保存失败");
      }
    }, 900);
    return () => clearTimeout(t);
  }, [md, draftId]);

  // 拟稿进行中：跟住后台任务，把阶段和已用时间显示出来。
  // 之前这里只有一个永不停的转圈，任务中断了也看不出来。
  React.useEffect(() => {
    if (!draftId || current?.status !== "researching") {
      setLiveJob(null);
      return;
    }
    let alive = true;
    let tries = 0;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      tries += 1;
      try {
        const { jobs } = await api.get<{ jobs: Job[] }>("/api/jobs");
        if (!alive) return;
        const job = jobs.find((j) => j.questionId === draftId && j.kind === "research");
        if (!job) {
          setLiveJob({ stage: "准备", message: "等待智能体接手…", state: "running", elapsed: 0 });
        } else {
          const started = Date.parse(job.startedAt);
          const elapsed = Number.isFinite(started)
            ? Math.max(0, Math.round((Date.now() - started) / 1000))
            : 0;
          setLiveJob({ stage: job.stage, message: job.message, state: job.state, elapsed });
          if (job.state === "failed") {
            toast(`拟稿失败：${job.message}`, "err");
            await qs.reload(true);
            return;
          }
          if (job.state === "done") {
            toast("初稿已就绪，可以去批阅了", "ok");
            await qs.reload(true);
            return;
          }
        }
      } catch {
        /* 单次轮询失败忽略 */
      }
      if (!alive || tries > 300) return; // 最多跟 10 分钟
      timer = setTimeout(tick, 2000);
    };
    timer = setTimeout(tick, 800);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftId, current?.status]);

  const words = md.replace(/\s/g, "").length;
  const min = se.data?.draftMin ?? 500;
  const max = se.data?.draftMax ?? 1000;
  const good = words >= min && words <= max;

  function noteCaret() {
    const ta = taRef.current;
    if (ta) caretRef.current = ta.selectionStart ?? 0;
  }

  /** 在插入点插入片段。编辑态跟着选区走，预览态用最后点中的段落位置。 */
  function insertAtCursor(snippet: string) {
    const ta = taRef.current;
    const inEditor = mode === "edit" && ta != null;
    const placed = caretRef.current != null || (inEditor && (ta!.selectionStart ?? 0) > 0);

    let start: number;
    let end: number;
    if (inEditor && placed) {
      start = ta!.selectionStart ?? 0;
      end = ta!.selectionEnd ?? start;
    } else if (caretRef.current != null) {
      start = caretRef.current;
      end = start;
    } else {
      // 从没在正文里落过光标，只能接在末尾
      start = md.length;
      end = md.length;
    }

    const next = md.slice(0, start) + snippet + md.slice(end);
    const caret = start + snippet.length;
    caretRef.current = caret;
    pendingCaret.current = caret;
    pendingScroll.current = ta?.scrollTop ?? null;
    dirty.current = true;
    setMd(next);
  }

  // 受控 textarea 更新 value 后浏览器会把光标甩到末尾，这里立刻按回插入点，
  // 并恢复滚动位置，避免插图时整篇往下跳。
  React.useLayoutEffect(() => {
    if (mode !== "edit") return;
    const pos = pendingCaret.current;
    if (pos == null) return;
    const ta = taRef.current;
    if (!ta) return;
    pendingCaret.current = null;
    ta.focus({ preventScroll: true });
    ta.setSelectionRange(pos, pos);
    if (pendingScroll.current != null) ta.scrollTop = pendingScroll.current;
    pendingScroll.current = null;
  }, [md, mode]);

  function insertImage(file: string, caption?: string) {
    insertAtCursor(`\n\n[IMG:${file}]\n\n`);
    toast(`已插入插图：${caption || file}`, "ok");
  }

  function exportText() {
    const byName = new Map(images.map((i) => [i.filename, i]));
    return md.replace(/\[IMG:([^\]]+)\]/g, (_, f) => {
      const a = byName.get(f);
      return `（插图：${a?.caption || f}）`;
    });
  }

  async function markAnswered() {
    if (!current) return;
    try {
      const r = await api.post<{ freedBytes?: number }>(`/api/questions/${current.id}/answered`);
      toast(`已标记回答，释放 ${Math.round((r.freedBytes ?? 0) / 1024)} KB`, "ok");
      setConfirm(null);
      await qs.reload(true);
      router.push("/desk");
    } catch (e) {
      toast(e instanceof Error ? e.message : "操作失败", "err");
    }
  }

  async function dismiss() {
    if (!current) return;
    try {
      await api.post(`/api/questions/${current.id}/dismiss`);
      toast("已弃用，本地素材已清理", "ok");
      setConfirm(null);
      await qs.reload(true);
      router.push("/desk");
    } catch (e) {
      toast(e instanceof Error ? e.message : "操作失败", "err");
    }
  }

  async function runAgent() {
    if (!current) return;
    try {
      await api.post(`/api/questions/${current.id}/research`);
      toast("智能体已接手", "ok");
      await qs.reload(true);
    } catch (e) {
      toast(e instanceof Error ? e.message : "启动失败", "err");
    }
  }

  if (!current) {
    return (
      <div className="desk">
        <div className="desk-side" />
        <div className="desk-main">
          <Empty glyph="案">
            案头是空的。去「奏章」里挑一个问题，点播放键让智能体先拟一稿。
          </Empty>
        </div>
      </div>
    );
  }

  const isPending = current.status === "pending_review";

  return (
    <div className="desk">
      {/* ---------------- 左：素材 ---------------- */}
      <aside className="desk-side">
        <div className="desk-side-head">
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 8,
              marginBottom: 8,
            }}
          >
            <span className="eyebrow" style={{ whiteSpace: "nowrap", flexShrink: 0 }}>
              素材案
            </span>
            <select
              className="select"
              style={{ flex: 1, minWidth: 0, fontSize: 11, padding: "3px 6px" }}
              value={current.id}
              onChange={(e) => router.push(`/desk?id=${e.target.value}`)}
            >
              {pending.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.status === "pending_review" ? "待批 · " : ""}
                  {p.title.slice(0, 22)}
                </option>
              ))}
            </select>
          </div>
          <div style={{ fontSize: 11.5, color: "var(--ink-3)" }}>
            {current.domain}
            {current.subDomain ? ` / ${current.subDomain}` : ""} · 契合{" "}
            {(current.fit * 100).toFixed(0)}
          </div>
        </div>

        <div className="desk-side-body">
          {loading && (
            <div style={{ padding: "10px 0", color: "var(--ink-3)", fontSize: 12 }}>
              <Spinner /> 正在取素材…
            </div>
          )}

          {!isPending && current.status !== "researching" && (
            <div
              style={{
                border: "1px solid var(--rule-2)",
                padding: 14,
                marginBottom: 16,
                background: "var(--cinnabar-wash)",
              }}
            >
              <p style={{ fontSize: 12.5, lineHeight: 1.8, color: "var(--ink-2)" }}>
                这件还没拟稿。可以让智能体先检索资料、校对、取图，再把草稿摆到你面前。
              </p>
              <button className="btn btn-red mt-8" onClick={runAgent}>
                让智能体开始拟稿 ▷
              </button>
            </div>
          )}

          {current.status === "researching" && (
            <div
              style={{
                border: "1px solid var(--rule-2)",
                padding: 14,
                marginBottom: 16,
                background: "var(--blue-wash)",
                fontSize: 12.5,
                color: "var(--ink-2)",
              }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <Spinner />
                <span>
                  智能体正在检索权威资料、核对疑问点、抓取配图…
                  {liveJob?.elapsed ? `（已用 ${liveJob.elapsed} 秒）` : ""}
                </span>
              </div>
              {liveJob?.message && (
                <p style={{ margin: "8px 0 0", fontSize: 11.5, color: "var(--ink-3)", lineHeight: 1.7 }}>
                  {liveJob.stage ? `${liveJob.stage} · ` : ""}
                  {liveJob.message}
                </p>
              )}
              <button
                className="btn btn-sm"
                style={{ marginTop: 10 }}
                onClick={() => {
                  void qs.reload(true);
                }}
              >
                <IconRefresh size={12} /> 刷新状态
              </button>
            </div>
          )}

          {warnings.length > 0 && (
            <div
              style={{
                border: "1px solid var(--rule-2)",
                borderLeft: "3px solid var(--cinnabar)",
                padding: "10px 12px",
                marginBottom: 16,
                background: "var(--cinnabar-wash)",
                fontSize: 11.5,
                lineHeight: 1.75,
                color: "var(--ink-2)",
              }}
            >
              <b style={{ display: "block", marginBottom: 4 }}>这一稿的已知问题</b>
              {warnings.map((w, i) => (
                <div key={i}>· {w}</div>
              ))}
            </div>
          )}

          <SectionTitle count={images.length} right={<span className="dim">拖动或点击插入</span>}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
              <IconImage size={13} /> 配图
            </span>
          </SectionTitle>

          {images.length === 0 ? (
            <p className="dim" style={{ fontSize: 11.5, marginBottom: 18, lineHeight: 1.7 }}>
              暂无配图。拟稿时会从你配置的权威平台挑几张备选，点一下插到光标处；正文本身不会自动带图。
            </p>
          ) : (
            <div className="img-grid" style={{ marginBottom: 20 }}>
              {images.map((img) => (
                <div
                  key={img.id}
                  className="img-card"
                  draggable
                  onDragStart={(e) => {
                    e.dataTransfer.setData("text/plain", `[IMG:${img.filename}]`);
                    e.dataTransfer.effectAllowed = "copy";
                  }}
                  onClick={() => insertImage(img.filename, img.caption)}
                  title={`点击插入 · ${img.sourceName}`}
                >
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img
                    src={imageSrc(current.id, img.filename)}
                    alt={img.caption}
                    draggable={false}
                  />
                  <div className="img-cap">{img.caption || img.filename}</div>
                </div>
              ))}
            </div>
          )}

          <div style={{ display: "flex", gap: 2, marginBottom: 10 }}>
            <button
              className="btn btn-sm"
              onClick={() => setTab("doubt")}
              style={{
                borderColor: tab === "doubt" ? "var(--ink-3)" : "transparent",
                background: tab === "doubt" ? "var(--paper-raised)" : "transparent",
                color: tab === "doubt" ? "var(--ink)" : "var(--ink-3)",
              }}
            >
              存疑 {doubts.length}
            </button>
            <button
              className="btn btn-sm"
              onClick={() => setTab("source")}
              style={{
                borderColor: tab === "source" ? "var(--ink-3)" : "transparent",
                background: tab === "source" ? "var(--paper-raised)" : "transparent",
                color: tab === "source" ? "var(--ink)" : "var(--ink-3)",
              }}
            >
              来源 {citations.length}
            </button>
          </div>

          {tab === "doubt" ? (
            doubts.length === 0 ? (
              <p className="dim" style={{ fontSize: 11.5, lineHeight: 1.7 }}>
                没有存疑点。这通常意味着资料足够扎实，但仍建议你自己扫一眼数值。
              </p>
            ) : (
              doubts.map((d, i) => (
                <div className="doubt" key={i}>
                  <p>{d.text}</p>
                  {d.verdict && (
                    <div className="verdict">
                      <b>
                        {d.status === "corrected"
                          ? "已更正 · "
                          : d.status === "rejected"
                            ? "已否定 · "
                            : "已核 · "}
                      </b>
                      {d.verdict}
                    </div>
                  )}
                </div>
              ))
            )
          ) : citations.length === 0 ? (
            <p className="dim" style={{ fontSize: 11.5, lineHeight: 1.7 }}>
              还没有引用来源。
            </p>
          ) : (
            citations.map((c, i) => (
              <div className="cite" key={i}>
                <span className="n">{i + 1}</span>
                <div style={{ minWidth: 0 }}>
                  <a
                    href={c.url}
                    onClick={(e) => {
                      e.preventDefault();
                      void openExternal(c.url);
                    }}
                  >
                    {c.label}
                  </a>
                  <div className="sn">{c.sourceName || citationHost(c.url)}</div>
                </div>
              </div>
            ))
          )}
        </div>
      </aside>

      {/* ---------------- 右：编辑器 ---------------- */}
      <section className="desk-main">
        <div style={{ padding: "16px 22px 12px", borderBottom: "1px solid var(--rule)" }}>
          <div style={{ display: "flex", alignItems: "flex-start", gap: 12 }}>
            <div style={{ minWidth: 0, flex: 1 }}>
              <h2
                style={{
                  fontFamily: "var(--font-kai)",
                  fontSize: 20,
                  fontWeight: 400,
                  lineHeight: 1.45,
                }}
              >
                {current.title}
              </h2>
              <div className="meta" style={{ marginTop: 5 }}>
                <span className="chip blue">{current.domain}</span>
                <span className="sep">·</span>
                <span>{current.askerName || "匿名"} 邀请</span>
                <span className="sep">·</span>
                <span>
                  <b>{current.answerCount}</b> 个回答
                </span>
                <span className="sep">·</span>
                <a
                  href={current.url}
                  onClick={(e) => {
                    e.preventDefault();
                    void openExternal(current.url);
                  }}
                >
                  在知乎查看 ↗
                </a>
              </div>
            </div>
            <div style={{ display: "flex", gap: 2 }}>
              <button
                className="btn btn-sm"
                onClick={() => setMode("edit")}
                style={{
                  borderColor: mode === "edit" ? "var(--ink-3)" : "transparent",
                  background: mode === "edit" ? "var(--paper)" : "transparent",
                }}
              >
                编辑
              </button>
              <button
                className="btn btn-sm"
                onClick={() => setMode("preview")}
                style={{
                  borderColor: mode === "preview" ? "var(--ink-3)" : "transparent",
                  background: mode === "preview" ? "var(--paper)" : "transparent",
                }}
              >
                预览
              </button>
            </div>
          </div>
        </div>

        {/* 编辑器常驻不卸载：切到预览时光标位置也不会丢，
            插图时才能准确插在原来那句话后面 */}
        <div className="desk-edit" style={mode === "edit" ? undefined : { display: "none" }}>
          <textarea
            ref={taRef}
            className="editor"
            value={md}
            placeholder="在这里落下你的判断。左侧配图点一下就能插到光标处，也可以直接拖进来。"
            onChange={(e) => {
              dirty.current = true;
              setMd(e.target.value);
            }}
            onSelect={noteCaret}
            onClick={noteCaret}
            onKeyUp={noteCaret}
            onBlur={noteCaret}
            onDragOver={(e) => {
              e.preventDefault();
              e.dataTransfer.dropEffect = "copy";
            }}
            onDrop={(e) => {
              e.preventDefault();
              const text = e.dataTransfer.getData("text/plain");
              if (text.startsWith("[IMG:")) {
                insertAtCursor(`\n\n${text}\n\n`);
                dirty.current = true;
              }
            }}
            spellCheck={false}
          />
        </div>

        {mode === "preview" && (
          <div className="desk-edit">
            <RenderMarkdown
              md={md}
              qid={current.id}
              images={images}
              onPick={(off) => {
                caretRef.current = off;
              }}
            />
          </div>
        )}

        <div className="desk-toolbar" style={{ borderTop: "1px solid var(--rule)", borderBottom: "none" }}>
          <span className="mono" style={{ color: good ? "var(--moss)" : "var(--ink-3)" }}>
            {words}
          </span>
          <span className="dim" style={{ fontSize: 11 }}>
            / 建议 {min}–{max} 字
          </span>
          <span className="dim" style={{ fontSize: 11, marginLeft: 8 }}>
            {savedAt}
          </span>
          <div className="grow" />
          <button
            className="btn btn-sm"
            onClick={() => {
              copy(exportText());
              toast("已复制全文，粘贴到知乎后按插图提示补齐图片即可", "ok");
            }}
          >
            <IconCopy size={13} /> 复制全文
          </button>
          <button
            className="btn btn-sm"
            onClick={() => openExternal(current.url)}
          >
            <IconExternal size={13} /> 去知乎作答
          </button>
          <button className="btn btn-sm" onClick={() => setConfirm("dismiss")}>
            <IconX size={13} /> 弃用
          </button>
          <button className="btn btn-sm btn-primary" onClick={() => setConfirm("answered")}>
            <IconCheck size={13} /> 标记已回答
          </button>
        </div>

        <div className="desk-foot">
          <IconQuill size={12} />
          <span>
            {current.hasDraft
              ? `初稿基于 ${citations.length} 条权威来源生成，${doubts.length} 处存疑已逐条核对`
              : "尚无初稿"}
          </span>
          <span className="dim">·</span>
          <span>落笔与定稿由你负责，这是本站的底线</span>
        </div>
      </section>

      {confirm === "answered" && (
        <Modal
          title="确认已经发布？"
          sub="标记后会归档正文、删除本地配图以释放空间。"
          onClose={() => setConfirm(null)}
        >
          <p style={{ fontSize: 12.5, lineHeight: 1.8, color: "var(--ink-2)" }}>
            请确认已经把这版内容贴到知乎并发布。归档的正文会保存在
            <code style={{ fontSize: 11.5 }}> data/domains/{current.domain}/ </code>
            下，之后仍可查阅。
          </p>
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 20 }}>
            <button className="btn" onClick={() => setConfirm(null)}>
              还没发
            </button>
            <button className="btn btn-primary" onClick={markAnswered}>
              已发布，归档
            </button>
          </div>
        </Modal>
      )}

      {confirm === "dismiss" && (
        <Modal title="弃用这件奏章？" sub={current.title} onClose={() => setConfirm(null)}>
          <p style={{ fontSize: 12.5, lineHeight: 1.8, color: "var(--ink-2)" }}>
            会删除本地已生成的正文与配图。如果只是暂时不想答，可以先留着。
          </p>
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 20 }}>
            <button className="btn" onClick={() => setConfirm(null)}>
              再想想
            </button>
            <button className="btn btn-red" onClick={dismiss}>
              确认弃用
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

export default function DeskPage() {
  return (
    <Suspense
      fallback={
        <div className="desk">
          <div className="desk-side" />
          <div className="desk-main">
            <div style={{ padding: 40 }}>
              <Spinner /> 正在开卷…
            </div>
          </div>
        </div>
      }
    >
      <DeskInner />
    </Suspense>
  );
}
