"use client";

import { useRouter } from "next/navigation";
import React from "react";
import type { Question } from "@/lib/types";
import { openExternal } from "@/lib/api";
import { relTime, FitMeter, IconPlay, IconQuill, IconTrash } from "./ui";

export default function MemorialRow({
  q,
  onResearch,
  onDismiss,
  compact,
}: {
  q: Question;
  onResearch?: (q: Question) => void;
  onDismiss?: (q: Question) => void;
  compact?: boolean;
}) {
  const router = useRouter();

  return (
    <article className="memorial" data-status={q.status}>
      <div className="memorial-score">
        <b className={q.fit < 0.6 ? "low" : ""}>{(q.fit * 100).toFixed(0)}</b>
        <span>契合</span>
        <FitMeter value={q.fit} />
      </div>

      <div className="memorial-body">
        <button
          className="memorial-title"
          onClick={() => router.push(`/desk?id=${q.id}`)}
          title="打开批阅台"
        >
          {q.title}
        </button>

        {!compact && q.excerpt && <p className="memorial-excerpt">{q.excerpt}</p>}

        <div className="meta">
          {q.status === "pending_review" && (
            <span className="stamp" style={{ height: 22, minWidth: 26, fontSize: 11 }}>
              待批
            </span>
          )}
          {q.domain && <span className="chip blue">{q.domain}</span>}
          {q.subDomain && <span className="chip">{q.subDomain}</span>}
          <span className="chip plain">{q.source === "invite" ? "邀请回答" : "推荐回答"}</span>
          <span className="sep">·</span>
          <span>
            <b>{q.answerCount}</b> 回答 · <b>{q.followerCount}</b> 关注
          </span>
          <span className="sep">·</span>
          <span>{q.askerName || "匿名用户"}</span>
          <span className="sep">·</span>
          <span>{relTime(q.syncedAt || q.createdAt)}</span>
          {q.hasDraft && (
            <>
              <span className="sep">·</span>
              <span className="chip moss">
                初稿 {q.wordCount} 字 · {q.imageCount} 图
              </span>
            </>
          )}
          {q.status === "researching" && (
            <span className="chip red">
              <span className="spin" style={{ width: 8, height: 8, borderWidth: 1 }} />
              智能体拟稿中
            </span>
          )}
        </div>

        {q.tags.length > 0 && (
          <div className="kw">
            {q.tags.slice(0, 5).map((t) => (
              <span key={t}>#{t}</span>
            ))}
          </div>
        )}
      </div>

      <div className="memorial-actions">
        {q.status === "pending_review" ? (
          <button
            className="play-btn"
            title="进入批阅台"
            onClick={() => router.push(`/desk?id=${q.id}`)}
            style={{ borderColor: "rgba(11,95,165,.45)", color: "var(--blue)" }}
          >
            <IconQuill size={12} />
          </button>
        ) : q.status === "researching" ? (
          <span className="stamp">拟稿</span>
        ) : (
          <button className="play-btn" title="让智能体开始拟稿" onClick={() => onResearch?.(q)}>
            <IconPlay size={11} />
          </button>
        )}
        {onDismiss && q.status !== "pending_review" && (
          <button className="btn-icon" title="弃用这个问题" onClick={() => onDismiss(q)}>
            <IconTrash size={14} />
          </button>
        )}
        <button
          className="btn-icon"
          title="在知乎打开"
          onClick={() => openExternal(q.url)}
        >
          ↗
        </button>
      </div>
    </article>
  );
}
