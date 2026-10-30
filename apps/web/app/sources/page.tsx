"use client";

import React from "react";
import {
  Empty,
  IconGlobe,
  IconRefresh,
  SectionTitle,
  Spinner,
  Stars,
  Toggle,
} from "@/components/ui";
import { api, toast } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import type { Source } from "@/lib/types";

const KIND_LABEL: Record<string, string> = {
  wiki: "维基",
  official: "官方",
  community: "社区",
  media: "媒体",
  dataset: "数据",
  forum: "论坛",
};

const USE_LABEL: Record<string, string> = {
  fact: "查事实",
  image: "取图",
  data: "查数据",
};

export default function SourcesPage() {
  const loaded = useLoad<{ sources: Source[] }>(() => api.get("/api/sources"));
  const [list, setList] = React.useState<Source[]>([]);
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    if (loaded.data) setList(loaded.data.sources);
  }, [loaded.data]);

  async function persist(next: Source[]) {
    setList(next);
    try {
      await api.put("/api/sources", { sources: next });
    } catch (e) {
      toast(e instanceof Error ? e.message : "保存失败", "err");
    }
  }

  function toggle(id: string, enabled: boolean) {
    persist(list.map((s) => (s.id === id ? { ...s, enabled } : s)));
  }

  function setTrust(id: string, trust: number) {
    persist(list.map((s) => (s.id === id ? { ...s, trust } : s)));
  }

  async function rediscover() {
    setBusy(true);
    try {
      await api.post("/api/sources/discover");
      toast("正在按领域重新检索权威平台", "ok");
      setTimeout(() => loaded.reload(true), 5000);
    } catch (e) {
      toast(e instanceof Error ? e.message : "启动失败", "err");
    } finally {
      setBusy(false);
    }
  }

  const domains = Array.from(new Set(list.map((s) => s.domain)));

  const body = (
    <>
      {list.length === 0 ? (
        <Empty glyph="源">
          还没有权威来源。做完领域画像后，系统会按领域自动检索维基、官方文档与垂直社区；
          你也可以先在这里等它跑完。
        </Empty>
      ) : (
        domains.map((d) => (
          <div key={d} style={{ marginBottom: 28 }}>
            <SectionTitle count={list.filter((s) => s.domain === d).length}>{d || "未分类"}</SectionTitle>
            <table className="table">
              <thead>
                <tr>
                  <th style={{ width: "30%" }}>平台</th>
                  <th style={{ width: 68 }}>类型</th>
                  <th style={{ width: 84 }}>可信度</th>
                  <th style={{ width: 120 }}>用途</th>
                  <th>说明</th>
                  <th style={{ width: 60 }} className="right">
                    启用
                  </th>
                </tr>
              </thead>
              <tbody>
                {list
                  .filter((s) => s.domain === d)
                  .map((s) => (
                    <tr key={s.id}>
                      <td>
                        <div className="name">{s.name}</div>
                        <a
                          className="url"
                          href={s.baseUrl}
                          target="_blank"
                          rel="noreferrer noopener"
                        >
                          {s.baseUrl.replace(/^https?:\/\//, "").replace(/\/$/, "")}
                        </a>
                      </td>
                      <td>
                        <span className="chip">{KIND_LABEL[s.kind] || s.kind}</span>
                      </td>
                      <td>
                        <button
                          title="点击调整可信度"
                          onClick={() => setTrust(s.id, s.trust >= 5 ? 3 : s.trust + 1)}
                          style={{ background: "transparent" }}
                        >
                          <Stars n={s.trust} />
                        </button>
                      </td>
                      <td>
                        <div style={{ display: "flex", gap: 3, flexWrap: "wrap" }}>
                          {s.useFor.map((u) => (
                            <span className="chip" key={u} style={{ fontSize: 10 }}>
                              {USE_LABEL[u] || u}
                            </span>
                          ))}
                        </div>
                      </td>
                      <td style={{ color: "var(--ink-3)", fontSize: 12, lineHeight: 1.65 }}>
                        {s.notes || "—"}
                      </td>
                      <td className="right">
                        <Toggle on={s.enabled} onChange={(v) => toggle(s.id, v)} />
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        ))
      )}
    </>
  );

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">
            到哪里查证 <small>SOURCES OF TRUTH</small>
          </h1>
          <p className="page-sub">
            拟稿时的取材范围。可信度越高越优先抓取，取图也只从这些站点拿。
          </p>
        </div>
        <div className="page-actions">
          <button className="btn" onClick={rediscover} disabled={busy}>
            {busy ? <Spinner /> : <IconRefresh size={14} />} 重新检索
          </button>
        </div>
      </header>

      <div className="page-body">

      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 14px",
          border: "1px solid var(--rule)",
          background: "linear-gradient(120deg, var(--paper-raised), transparent)",
          marginBottom: 24,
          fontSize: 12,
          color: "var(--ink-3)",
          lineHeight: 1.7,
        }}
      >
        <IconGlobe size={15} />
        <span>
          只有勾选的来源会被抓取。关掉某个站点后，已生成的稿子不会变，但下次拟稿不再引用它。
        </span>
      </div>

      {body}
    </div>
    </main>
  );
}
