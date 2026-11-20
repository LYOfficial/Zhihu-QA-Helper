"use client";

import React from "react";
import { api } from "@/lib/api";
import { Modal, Spinner } from "./ui";

type Result = { url: string; id?: string; error?: string };

export default function ImportQuestions({ onImported }: { onImported: () => void }) {
  const [open, setOpen] = React.useState(false);
  const [text, setText] = React.useState("");
  const [results, setResults] = React.useState<Result[]>([]);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const close = () => { if (!busy) setOpen(false); };

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (busy || !text.trim()) return;
    setBusy(true);
    setError("");
    setResults([]);
    try {
      const response = await api.post<{ results: Result[] }>("/api/questions/import", { url: text });
      setResults(response.results);
      if (response.results.some((item) => item.id)) onImported();
    } catch (error) {
      setError(error instanceof Error ? error.message : "导入失败，请重试");
    } finally {
      setBusy(false);
    }
  }

  return <>
    <button className="btn" onClick={() => { setOpen(true); setText(""); setResults([]); setError(""); }}>＋ 导入问题</button>
    {open && <Modal title="导入知乎问题" sub="可粘贴知乎分享时复制的整段文本，每次最多 10 条链接。" width={540} onClose={close}>
      <form onSubmit={submit}>
        <div className="field">
          <label htmlFor="question-share-links">问题或回答链接</label>
          <textarea id="question-share-links" className="input import-text" rows={5} maxLength={16000} autoFocus
            value={text} disabled={busy} onChange={(event) => setText(event.target.value)}
            placeholder={"如何评价某部作品？ - 知乎\nhttps://www.zhihu.com/question/123456789\nhttps://www.zhihu.com/answer/987654321"} />
        </div>
        <p className="hint">回答链接会定位到原问题；专栏文章不会导入。导入成功的问题会置顶，不会自动拟稿。</p>
        {error && <p role="alert">{error}</p>}
        {results.length > 0 && <div className="import-results" role="status">
          <strong>已导入或置顶 {results.filter((item) => item.id).length} 个问题</strong>
          {results.filter((item) => item.error).map((item) => <p key={item.url} className="hint">{item.error}：{item.url}</p>)}
        </div>}
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={busy} onClick={close}>{results.length ? "完成" : "取消"}</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !text.trim()}>{busy ? <><Spinner /> 正在读取知乎</> : "导入并置顶"}</button>
        </div>
      </form>
    </Modal>}
  </>;
}
