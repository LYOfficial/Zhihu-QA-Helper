"use client";

import React from "react";

/* ---------------------------------------------------------------- 图标 */
type P = { size?: number; className?: string };
const svg = (d: React.ReactNode, vb = "0 0 24 24") =>
  function Icon({ size = 18, className }: P) {
    return (
      <svg
        width={size}
        height={size}
        viewBox={vb}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
        className={className}
        aria-hidden="true"
      >
        {d}
      </svg>
    );
  };

export const IconDesk = svg(
  <>
    <path d="M3 8h18M5 8v11h14V8" />
    <path d="M8 8V5h8v3M9 13h6" />
  </>
);
export const IconInbox = svg(
  <>
    <path d="M4 5h16v14H4z" />
    <path d="M4 14h4l1.5 2h5L16 14h4" />
  </>
);
export const IconQuill = svg(
  <>
    <path d="M20 4c-6 1-11 5-13 10l-1 4 4-1c5-2 9-7 10-13z" />
    <path d="M6 18l7-7" />
  </>
);
export const IconCompass = svg(
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M15 9l-2 4-4 2 2-4z" />
  </>
);
export const IconSliders = svg(
  <>
    <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
    <circle cx="15" cy="7" r="2" />
    <circle cx="9" cy="17" r="2" />
  </>
);
export const IconChart = svg(
  <>
    <path d="M4 19V5M4 19h16" />
    <path d="M8 19v-6M12 19V9M16 19v-4" />
  </>
);
export const IconPlay = svg(<path d="M7 4l13 8-13 8z" fill="currentColor" stroke="none" />);
export const IconCheck = svg(<path d="M4 12.5l5 5L20 6.5" />);
export const IconX = svg(<path d="M6 6l12 12M18 6L6 18" />);
export const IconRefresh = svg(
  <>
    <path d="M20 11a8 8 0 10-2.3 5.7" />
    <path d="M20 5v6h-6" />
  </>
);
export const IconCopy = svg(
  <>
    <rect x="9" y="9" width="11" height="11" rx="1.5" />
    <path d="M15 9V5.5A1.5 1.5 0 0013.5 4h-9A1.5 1.5 0 003 5.5v9A1.5 1.5 0 004.5 16H9" />
  </>
);
export const IconExternal = svg(
  <>
    <path d="M14 4h6v6" />
    <path d="M20 4l-9 9" />
    <path d="M18 14v4.5A1.5 1.5 0 0116.5 20h-11A1.5 1.5 0 014 18.5v-11A1.5 1.5 0 015.5 6H10" />
  </>
);
export const IconSearch = svg(
  <>
    <circle cx="11" cy="11" r="6.5" />
    <path d="M16 16l4 4" />
  </>
);
export const IconTrash = svg(
  <>
    <path d="M4 7h16M9 7V4.5h6V7M6 7l1 13h10l1-13" />
  </>
);
export const IconImage = svg(
  <>
    <rect x="3" y="5" width="18" height="14" rx="1.5" />
    <circle cx="8.5" cy="10" r="1.5" />
    <path d="M4 17l5-4 3.5 3 3-2.5L20 17" />
  </>
);
export const IconGlobe = svg(
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M3.5 12h17M12 3.5c2.5 2.5 2.5 14.5 0 17-2.5-2.5-2.5-14.5 0-17z" />
  </>
);
export const IconSpark = svg(
  <>
    <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" />
    <path d="M18.5 16.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z" />
  </>
);
export const IconCheckCircle = svg(
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M8 12.2l2.6 2.6L16 9.4" />
  </>
);
export const IconClock = svg(
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 7.5V12l3 2" />
  </>
);
export const IconArrowRight = svg(<path d="M4 12h15M14 6l6 6-6 6" />);
export const IconUser = svg(
  <>
    <circle cx="12" cy="8.5" r="3.8" />
    <path d="M4.5 20c.8-3.8 3.8-5.8 7.5-5.8s6.7 2 7.5 5.8" />
  </>
);

/* ---------------------------------------------------------------- 品牌标记 */

export function Seal({ size = 40 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 128 128" aria-label="知乎答主助手" role="img">
      <rect x="6" y="6" width="116" height="116" rx="32" fill="currentColor" />
      <g transform="translate(64 64) scale(.8) translate(-59 -69)">
        <path
          d="M28 17h52l20 20v74c0 5.523-4.477 10-10 10H28c-5.523 0-10-4.477-10-10V27c0-5.523 4.477-10 10-10Z"
          fill="white"
        />
        <path d="M80 17v20h20" fill="#dce9ff" />
        <path d="M36 40h30M36 48h18M36 100h56" fill="none" stroke="currentColor" strokeWidth="5" strokeLinecap="round" />
        <g transform="translate(48 55) scale(.4)" fill="currentColor">
          <path d="m53.29 80.035 7.32.002 2.41 8.24 13.128-8.24h15.477v-67.98h-38.335zm7.79-60.598h22.756v53.22h-8.73l-8.718 5.473-1.587-5.46-3.72-.012v-53.22z" />
          <path d="m46.818 43.162h-16.35c.545-8.467.687-16.12.687-22.955h15.987s.615-7.05-2.68-6.97h-27.655c1.09-4.1 2.46-8.332 4.1-12.708 0 0-7.523 0-10.085 6.74-1.06 2.78-4.128 13.48-9.592 24.41 1.84-.2 7.927-.37 11.512-6.94.66-1.84.785-2.08 1.605-4.54h9.02c0 3.28-.374 20.9-.526 22.95h-16.331c-3.67 0-4.863 7.38-4.863 7.38h20.493c-1.375 15.581-8.755 28.711-22.14 39.091 6.403 1.828 12.784-.29 15.937-3.094 0 0 7.182-6.53 11.12-21.64l16.863 20.294s2.473-8.402-.388-12.496c-2.37-2.788-8.768-10.33-11.496-13.064l-4.57 3.627c1.363-4.368 2.183-8.61 2.46-12.71h19.264s-.027-7.38-2.372-7.38z" />
        </g>
      </g>
    </svg>
  );
}

/* ---------------------------------------------------------------- 基础件 */

export function FitMeter({ value, blue }: { value: number; blue?: boolean }) {
  const on = Math.round(Math.max(0, Math.min(1, value)) * 5);
  return (
    <div className={`fit-meter${blue ? " blue" : ""}`} title={`契合度 ${(value * 100).toFixed(0)}%`}>
      {[0, 1, 2, 3, 4].map((i) => (
        <i key={i} className={i < on ? "on" : ""} />
      ))}
    </div>
  );
}

export function StatusStamp({ status }: { status: string }) {
  const map: Record<string, { text: string; cls: string }> = {
    inbox: { text: "待阅", cls: "grey" },
    researching: { text: "拟稿", cls: "" },
    pending_review: { text: "待批", cls: "" },
    answered: { text: "已答", cls: "moss" },
    dismissed: { text: "弃用", cls: "grey" },
  };
  const s = map[status] || { text: status, cls: "grey" };
  return <span className={`stamp ${s.cls}`}>{s.text}</span>;
}

export function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      className="toggle"
      data-on={on}
      aria-pressed={on}
      onClick={() => onChange(!on)}
    />
  );
}

export function SectionTitle({
  children,
  count,
  right,
}: {
  children: React.ReactNode;
  count?: number | string;
  right?: React.ReactNode;
}) {
  return (
    <div className="section-title">
      <span>{children}</span>
      {count !== undefined && <span className="count">{count}</span>}
      {right && <span className="right">{right}</span>}
    </div>
  );
}

export function Empty({ glyph, children }: { glyph?: string; children: React.ReactNode }) {
  return (
    <div className="empty">
      <div className="glyph">{glyph || "空"}</div>
      <p>{children}</p>
    </div>
  );
}

export function Spinner() {
  return <span className="spin" />;
}

export function Stars({ n }: { n: number }) {
  return (
    <span className="stars">
      {[1, 2, 3, 4, 5].map((i) => (
        <i key={i} style={{ color: i <= n ? "inherit" : undefined }}>
          ★
        </i>
      ))}
    </span>
  );
}

export function Modal({
  title,
  sub,
  onClose,
  children,
  width,
}: {
  title: string;
  sub?: string;
  onClose: () => void;
  children: React.ReactNode;
  width?: number;
}) {
  React.useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);

  return (
    <div className="overlay" onClick={onClose}>
      <div
        className="modal"
        style={width ? { width: `min(${width}px, 100%)` } : undefined}
        onClick={(e) => e.stopPropagation()}
      >
        <h3>{title}</h3>
        {sub && <p className="sub">{sub}</p>}
        <button className="btn-icon modal-close" onClick={onClose} aria-label="关闭">
          <IconX size={16} />
        </button>
        {children}
      </div>
    </div>
  );
}

/* ---------------------------------------------------------------- 时间 */

export function relTime(iso?: string | null): string {
  if (!iso) return "—";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "—";
  const diff = Date.now() - t;
  if (diff < 0) {
    const m = Math.round(-diff / 60000);
    if (m < 60) return `${m} 分钟后`;
    return `${Math.round(m / 60)} 小时后`;
  }
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d} 天前`;
  return new Date(iso).toLocaleDateString("zh-CN");
}

export function today(): string {
  const d = new Date();
  const week = ["日", "一", "二", "三", "四", "五", "六"][d.getDay()];
  return `${d.getMonth() + 1} 月 ${d.getDate()} 日 · 周${week}`;
}
