"use client";

import { useRouter, useSearchParams } from "next/navigation";
import React, { Suspense } from "react";
import { createPortal } from "react-dom";
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

function IllustrationTools({ question, disabled, onImages }: {
  question: Question;
  disabled: boolean;
  onImages: (id: string, assets: Draft["images"]) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const [sourceUrl, setSourceUrl] = React.useState("");
  const [file, setFile] = React.useState<File | null>(null);
  const [busy, setBusy] = React.useState<"search" | "import" | null>(null);
  const [notice, setNotice] = React.useState("");
  const base = `/api/images/${encodeURIComponent(question.id)}`;
  const keywords = query.trim();

  async function run(action: "search" | "import") {
    if (busy || disabled) return;
    if (action === "import" && (!file || file.size > 6_000_000)) {
      setNotice("请选择不超过 6 MB 的 PNG、JPEG 或 WebP 图片。");
      return;
    }
    setBusy(action);
    setNotice(action === "search" ? "正在提取画面关键词、查找原图并去重，可能需要约 40 秒。" : "正在检查图片并去重…");
    try {
      let result: { added: number };
      if (action === "search") {
        result = await api.post(base, { query: query.trim() });
      } else if (file) {
        const imageData = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onerror = () => reject(new Error("无法读取图片文件"));
          reader.onload = () => resolve(String(reader.result).split(",")[1] || "");
          reader.readAsDataURL(file);
        });
        result = await api.post(`${base}/import`, { imageData, sourceUrl, caption: file.name.replace(/\.[^.]+$/, "") });
      } else {
        return;
      }
      onImages(question.id, await api.get<Draft["images"]>(base));
      setNotice(result.added ? `已加入 ${result.added} 张备选图，正文未改动。` : action === "import"
        ? "已有相同或相似的图片，未重复添加。"
        : "没有新增合适的配图。试试更短的关键词、英文名称，或在浏览器找到后导入。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : "配图操作未完成，请重试。");
    } finally {
      setBusy(null);
    }
  }

  return <>
    <button className="btn btn-sm" onClick={() => setOpen(true)} disabled={disabled}>找配图</button>
    {open && createPortal(<Modal title="找配图" sub="图片只加入备选，不改正文。使用前请核对可见水印、出处和转载许可。"
      onClose={() => { if (!busy) setOpen(false); }} width={490}>
      <div className="illustration-tools">
        <label>搜图关键词或公开帖子链接 <input className="input" value={query} onChange={(event) => setQuery(event.target.value)}
          placeholder="留空自动拆词，也可输入想找的具体画面" maxLength={2048} disabled={!!busy} /></label>
        <div className="illustration-actions">
          <button className="btn btn-sm" disabled={disabled || !!busy} onClick={() => void run("search")}>
            {busy === "search" ? "正在搜图…" : "自动搜图"}
          </button>
          <button className="btn btn-sm" disabled={!keywords} onClick={() => void openExternal(keywords.startsWith("https://") ? keywords : `https://www.bing.com/images/search?q=${encodeURIComponent(keywords)}`)}>
            <IconExternal size={13} /> 浏览器搜图
          </button>
        </div>
        <p className="dim">支持全网搜图、Wikimedia Commons，也可粘贴百度贴吧、知乎或小黑盒的公开帖子链接。打不开或明确带水印的候选会跳过。</p>
        <label>导入本地图片 <input className="input" type="file" accept="image/png,image/jpeg,image/webp"
          onChange={(event) => setFile(event.target.files?.[0] ?? null)} disabled={!!busy} /></label>
        <label>来源网页（可选） <input className="input" type="url" value={sourceUrl} maxLength={2048}
          onChange={(event) => setSourceUrl(event.target.value)} placeholder="https://…" disabled={!!busy} /></label>
        <div><button className="btn btn-sm" disabled={!file || disabled || !!busy} onClick={() => void run("import")}>
          {busy === "import" ? "正在导入…" : "加入备选"}
        </button></div>
        {notice && <p className="dim" role="status">{notice}</p>}
      </div>
    </Modal>, document.body)}
  </>;
}

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

type ResearchStatus = { job: Job | null; resumable: boolean; draftSaved: boolean };

function recoveryKey(id: string) {
  return `zhihu-qa:draft-recovery:${id}`;
}

const writes = new Map<string, Promise<unknown>>();

function persistDraft(id: string, markdown: string) {
  const previous = writes.get(id);
  const write = (previous ?? Promise.resolve()).catch(() => {}).then(async () => {
    await api.put(`/api/drafts/${id}`, { markdown });
    try {
      if (localStorage.getItem(recoveryKey(id)) === markdown) localStorage.removeItem(recoveryKey(id));
    } catch {}
  });
  writes.set(id, write);
  return write.finally(() => { if (writes.get(id) === write) writes.delete(id); });
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
  const [researchStatus, setResearchStatus] = React.useState<ResearchStatus | null>(null);
  const liveJob = researchStatus?.job;
  const [loadedId, setLoadedId] = React.useState("");
  const [draftError, setDraftError] = React.useState("");
  const [starting, setStarting] = React.useState(false);
  const [refreshKey, refresh] = React.useReducer((value) => value + 1, 0);
  const [mode, setMode] = React.useState<"edit" | "preview">("preview");
  const [savedAt, setSavedAt] = React.useState<string>("");
  const [loading, setLoading] = React.useState(false);
  const [confirm, setConfirm] = React.useState<null | "answered" | "dismiss">(null);
  const [tab, setTab] = React.useState<"doubt" | "source">("doubt");
  const taRef = React.useRef<HTMLTextAreaElement>(null);
  const dirty = React.useRef(false);
  const serverMarkdown = React.useRef("");
  // 记住最后一次落下的插入点：编辑态是选区，预览态是点中的段落结尾。
  // 之前预览态直接 append 到 md 末尾，插图永远跑到文末，这就是那个 bug。
  const caretRef = React.useRef<number | null>(null);
  const pendingCaret = React.useRef<number | null>(null);
  const pendingScroll = React.useRef<number | null>(null);
  const { copy } = useCopy();

  const draftId = current?.id ?? "";
  const viewedDraft = React.useRef(draftId);
  viewedDraft.current = draftId;

  React.useEffect(() => {
    if (!draftId) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    let first = true;
    let marker = "";
    dirty.current = false;
    caretRef.current = null;
    pendingCaret.current = null;
    setResearchStatus(null);
    setDraftError("");
    setLoading(true);
    setLoadedId("");
    const update = async () => {
      let nextDelay = 0;
      try {
        const status = await api.get<ResearchStatus>(`/api/questions/${draftId}/research`);
        if (!alive) return;
        setResearchStatus(status);
        nextDelay = status.job?.state === "running" ? 1600 : 6000;
        const currentMarker = `${status.job?.id}:${status.job?.state}:${status.draftSaved}`;
        if (first || currentMarker !== marker) {
          await writes.get(draftId)?.catch(() => {});
          const data = await api.get<Draft>(`/api/drafts/${draftId}`);
          if (!alive) return;
          let recovered: string | null = null;
          try { recovered = localStorage.getItem(recoveryKey(draftId)); } catch {}
          serverMarkdown.current = data.markdown || "";
          if (first || !dirty.current) {
            setMd(recovered ?? serverMarkdown.current);
            if (first) setMode("edit");
            if (recovered !== null) dirty.current = true;
            setSavedAt(recovered !== null ? "已恢复本机未提交的改动"
              : serverMarkdown.current ? "已保存" : "");
          }
          setImages(data.images || []);
          setDoubts(data.doubts || []);
          setCitations(data.citations || []);
          setWarnings(data.warnings || []);
          setLoadedId(draftId);
          if (!first && status.job?.state !== "running") void qs.reload(true);
          first = false;
          marker = currentMarker;
        }
        setDraftError("");
      } catch (error) {
        if (!alive) return;
        setDraftError(error instanceof Error ? error.message : "暂时无法读取工作台");
        nextDelay = 4000;
      } finally {
        if (alive) {
          setLoading(false);
          if (nextDelay) timer = setTimeout(update, nextDelay);
        }
      }
    };
    void update();
    return () => { alive = false; clearTimeout(timer); };
  }, [draftId, refreshKey, qs.reload]);

  React.useEffect(() => {
    if (!draftId || loadedId !== draftId || !dirty.current) return;
    let alive = true;
    const timer = setTimeout(() => {
      void persistDraft(draftId, md).then(() => {
        if (alive) setSavedAt("已保存");
      }).catch(() => {
        if (alive) setSavedAt("保存失败 · 点击重试");
      });
    }, 550);
    return () => { alive = false; clearTimeout(timer); };
  }, [md, draftId, loadedId, refreshKey]);

  function editMarkdown(markdown: string) {
    if (!draftId || loadedId !== draftId) return;
    dirty.current = true;
    try {
      localStorage.setItem(recoveryKey(draftId), markdown);
      setSavedAt("待同步 · 本机已有备份");
    } catch {
      setSavedAt("待保存");
    }
    setMd(markdown);
  }

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
    editMarkdown(next);
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
    if (md.includes(`[IMG:${file}]`)) {
      toast("这张图已经在正文中", "info");
      return;
    }
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
      if (dirty.current) await persistDraft(current.id, md);
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
      await writes.get(current.id)?.catch(() => {});
      await api.post(`/api/questions/${current.id}/dismiss`);
      try { localStorage.removeItem(recoveryKey(current.id)); } catch {}
      toast("已弃用，本地素材已清理", "ok");
      setConfirm(null);
      await qs.reload(true);
      router.push("/desk");
    } catch (e) {
      toast(e instanceof Error ? e.message : "操作失败", "err");
    }
  }

  async function runAgent() {
    if (!current || starting) return;
    setStarting(true);
    try {
      await api.post(`/api/questions/${current.id}/research`);
      refresh();
      void qs.reload(true);
    } catch (e) {
      toast(e instanceof Error ? e.message : "启动失败", "err");
    } finally {
      setStarting(false);
    }
  }

  if (!current) {
    return (
      <div className="desk">
        <div className="desk-side" />
        <div className="desk-main">
          <Empty glyph="案">
            还没有要处理的问题。到「问题收件箱」选择一条，或先同步知乎邀请。
          </Empty>
        </div>
      </div>
    );
  }

  const processing = liveJob?.state === "running" || starting || (!researchStatus && current.status === "researching");
  const canEdit = loadedId === draftId && (!processing || !!serverMarkdown.current);
  const progress = Math.round((liveJob?.progress ?? 0) * 100);
  const canStart = !processing && loadedId === draftId && (!!researchStatus?.resumable || (!current.hasDraft && !md.trim()));

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
              资料与配图
            </span>
            <select
              className="select"
              style={{ flex: 1, minWidth: 0, fontSize: 11, padding: "3px 6px" }}
              value={current.id}
              aria-label="切换问题"
              onChange={(e) => router.push(`/desk?id=${encodeURIComponent(e.target.value)}`)}
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
              <b style={{ display: "block", marginBottom: 4 }}>本次拟稿提示</b>
              {warnings.map((w, i) => (
                <div key={i}>· {w}</div>
              ))}
            </div>
          )}

          <SectionTitle count={images.length} right={<IllustrationTools key={draftId} question={current}
            disabled={processing || loadedId !== draftId}
            onImages={(id, assets) => { if (viewedDraft.current === id) setImages(assets); }} />}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
              <IconImage size={13} /> 配图
            </span>
          </SectionTitle>

          {images.length === 0 ? (
            <p className="dim" style={{ fontSize: 11.5, marginBottom: 18, lineHeight: 1.7 }}>
              暂无配图。可以点击「找配图」按画面搜图，或导入自己找到的图片；补搜不会改动正文。
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
                  {img.sourceUrl && <button className="illustration-source" title={img.sourceName}
                    onClick={(event) => { event.stopPropagation(); void openExternal(img.sourceUrl); }}>
                    <IconExternal size={11} /> {img.sourceName || "查看出处"}
                  </button>}
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
              核查 {doubts.length}
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
                没有逐项核查记录，不代表正文事实已全部核实。
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
                            : d.status === "verified" && d.source ? "资料支持 · " : "未核实 · "}
                      </b>
                      {d.verdict}
                      {d.confidence != null && (
                        <div className="dim" style={{ marginTop: 4 }}>
                          {d.confidence ? `证据把握 ${d.confidence}%（助手估计，非实测准确率）` : "证据不足，无法估计把握"}
                        </div>
                      )}
                      {d.source && !d.evidence?.length && <button className="btn btn-sm" onClick={() => void openExternal(d.source)}>查看核查来源</button>}
                      {d.evidence?.map((entry) => (
                        <details key={entry.url} style={{ marginTop: 6 }}>
                          <summary>{entry.stance === "supports" ? "支持" : entry.stance === "refutes" ? "反证" : "背景"} · {entry.sourceName}</summary>
                          <button className="btn btn-sm" style={{ maxWidth: "100%", height: "auto", whiteSpace: "normal", overflowWrap: "anywhere", textAlign: "left" }}
                            onClick={() => void openExternal(entry.url)}>{entry.label || "阅读原文"}</button>
                          <div className="dim">{({ official: "官方/发布方", docs: "原始文档", wiki: "维基", media: "媒体", community: "社区个案" } as Record<string, string>)[entry.kind] || "网页"}{entry.publishedAt ? ` · ${entry.publishedAt.slice(0, 10)}` : ""}</div>
                          <blockquote style={{ margin: "5px 0", overflowWrap: "anywhere" }}>{entry.quote}</blockquote>
                        </details>
                      ))}
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
                  <div className="sn">{c.sourceName || citationHost(c.url)}{c.snippet ? ` · ${c.snippet}` : ""}</div>
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

        {(processing || canStart || liveJob?.state === "failed" || draftError) && (
          <section className="research-card" aria-live="polite">
            <div className="research-card-title">
              {processing && <Spinner />}
              <strong>{processing ? liveJob?.stage || "正在排队" : researchStatus?.resumable ? "有可继续的任务" : draftError ? "暂时无法连接" : "准备拟稿"}</strong>
              {processing && <span className="mono">{progress}%</span>}
            </div>
            {processing && <progress max={100} value={progress} aria-label="拟稿进度" />}
            <p>{draftError || liveJob?.message || "先保存资料和正文，再补充核查与配图。"}</p>
            {canStart && <button className="btn btn-primary btn-sm" onClick={runAgent}>
              {researchStatus?.resumable ? "继续上次任务" : "生成初稿"}
            </button>}
          </section>
        )}
        {/* 编辑器常驻不卸载：切到预览时光标位置也不会丢，
            插图时才能准确插在原来那句话后面 */}
        <div className="desk-edit" style={mode === "edit" ? undefined : { display: "none" }}>
          <textarea
            ref={taRef}
            className="editor"
            value={loadedId === draftId ? md : ""}
            disabled={!canEdit}
            aria-label="回答正文"
            placeholder="在这里落下你的判断。左侧配图点一下就能插到光标处，也可以直接拖进来。"
            onChange={(e) => {
              editMarkdown(e.target.value);
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
              const asset = images.find((image) => text === `[IMG:${image.filename}]`);
              if (asset) insertImage(asset.filename, asset.caption);
            }}
            spellCheck={false}
          />
        </div>

        {mode === "preview" && (
          <div className="desk-edit">
            <RenderMarkdown
              md={loadedId === draftId ? md : ""}
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
            {savedAt.startsWith("保存失败") ? (
              <button className="btn btn-sm" onClick={() => void persistDraft(draftId, md).then(
                () => setSavedAt("已保存"), () => toast("仍无法保存，正文保留在本机恢复区", "err"),
              )}>{savedAt}</button>
            ) : savedAt}
          </span>
          <div className="grow" />
          <button
            className="btn btn-sm"
            disabled={loadedId !== draftId || !md.trim()}
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
          <button className="btn btn-sm" disabled={processing || loadedId !== draftId} onClick={() => setConfirm("dismiss")}>
            <IconX size={13} /> 弃用
          </button>
          <button className="btn btn-sm btn-primary" disabled={processing || !canEdit || !md.trim()} onClick={() => setConfirm("answered")}>
            <IconCheck size={13} /> 标记已回答
          </button>
        </div>

        <div className="desk-foot">
          <IconQuill size={12} />
          <span>
            {md.trim() ? `${citations.length} 条引用 · ${doubts.filter((d) => !d.source || d.status === "unverified" || d.status === "open").length} 处待核实` : "尚无正文"}
          </span>
          <span className="dim">·</span>
          <span>仅保存本地草稿，不会代你发布</span>
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
        <Modal title="弃用这个问题？" sub={current.title} onClose={() => setConfirm(null)}>
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
