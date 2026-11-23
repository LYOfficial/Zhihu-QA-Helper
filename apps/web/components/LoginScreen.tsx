"use client";

import React from "react";
import { api, toast } from "@/lib/api";
import { IconExternal, Spinner } from "./ui";

export default function LoginScreen({
  onSuccess,
}: {
  onSuccess: () => void;
}) {
  const [waiting, setWaiting] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!waiting) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const { status } = await api.post<{ status: string }>("/api/session/oauth/poll");
        if (!active) return;
        if (status === "pending") {
          setError("");
          timer = setTimeout(poll, 1500);
          return;
        }
        setWaiting(false);
        if (status === "complete") {
          toast("知乎授权成功", "ok");
          onSuccess();
        } else if (status === "cancelled") {
          setError("已取消知乎授权，可重新登录。");
        } else if (status === "expired") {
          setError("授权等待已结束，请重新发起登录。");
        } else {
          setError("知乎授权或账号验证失败，请重试。");
        }
      } catch (e) {
        if (!active) return;
        setError(e instanceof Error ? e.message : "暂时无法检查授权状态。");
        timer = setTimeout(poll, 2500);
      }
    }
    timer = setTimeout(poll, 1500);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [waiting, onSuccess]);

  async function authorize() {
    setBusy(true);
    setError("");
    try {
      await api.post("/api/session/oauth/start");
      setWaiting(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : "无法打开知乎授权页。");
    } finally {
      setBusy(false);
    }
  }

  async function cancel() {
    setWaiting(false);
    setBusy(true);
    setError("");
    try {
      await api.post("/api/session/oauth/cancel");
      onSuccess();
    } catch (e) {
      setError(e instanceof Error ? e.message : "无法取消，请重试。");
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="login">
      <div className="login-brand">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img className="login-logo" src="/logo.svg" alt="" width={76} height={76} />
        <h1>知乎答主助手</h1>
        <p>发现值得回答的问题，查资料，写初稿。</p>
      </div>
      <div className="login-card">
        {waiting ? (
          <>
            <div className="login-waiting" role="status">
              <Spinner /> 请在浏览器完成知乎授权
            </div>
            <button className="btn btn-block" onClick={cancel}>取消授权等待</button>
          </>
        ) : (
          <button className="btn btn-primary btn-block" disabled={busy} onClick={authorize}>
            {busy ? <Spinner /> : <IconExternal size={16} />} 使用知乎账号登录
          </button>
        )}
        {error && <div className="login-err" role="alert">{error}</div>}
      </div>
    </main>
  );
}
