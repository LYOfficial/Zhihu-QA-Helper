"use client";

import React from "react";
import { api, toast } from "@/lib/api";
import type { Theme } from "@/lib/theme";
import { IconExternal, Spinner, Seal } from "./ui";

export default function LoginScreen({
  onSuccess,
  theme,
  onThemeChange,
}: {
  onSuccess: () => void;
  theme: Theme;
  onThemeChange: (theme: Theme) => void;
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
    <div className="login">
      <div className="login-brand">
        <div className="seal">
          <Seal size={46} />
        </div>
        <h1>知乎答主助手</h1>
        <p className="lead">
          把「今天该答哪个问题」变成一份摊在案头的奏章。
          <br />
          AI 只备料，落笔永远是你。
        </p>

        <ul className="login-points">
          <li>
            <span className="idx">一</span>
            <span>
              <b>收集适合回答的问题</b>
              按领域找题，也可以直接导入知乎问题链接。
            </span>
          </li>
          <li>
            <span className="idx">二</span>
            <span>
              <b>联网取材与核对</b>
              只从你认可的权威平台抓资料与配图，生成的每个数值都要能追到原文。
            </span>
          </li>
          <li>
            <span className="idx">三</span>
            <span>
              <b>初稿摆上案头，由你定稿</b>
              不留 AI 直接发布的入口。存疑处逐条列出，改不改由你决定。
            </span>
          </li>
        </ul>

        <div className="foot">
          <span>v0.1</span>
          <span>草稿和资料保存在本机</span>
        </div>
      </div>

      <div className="login-right">
        <button
          className="login-theme-toggle"
          onClick={() => onThemeChange(theme === "paper" ? "zhihu" : "paper")}
        >
          切换到{theme === "paper" ? "现代主题" : "纸质主题"}
        </button>
        <div className="login-card">
          <h2>登录知乎</h2>
          <p className="sub">在浏览器的知乎授权页确认后，返回此处即可继续。</p>

          {waiting ? (
            <>
              <div className="login-waiting" role="status">
                <Spinner />
                <span>正在等待知乎授权</span>
              </div>
              <button className="btn btn-block" onClick={cancel}>取消授权等待</button>
            </>
          ) : (
            <button
              className="btn btn-primary btn-block"
              style={{ marginTop: 18 }}
              disabled={busy}
              onClick={authorize}
            >
              {busy ? <Spinner /> : <IconExternal size={14} />} 使用知乎账号登录
            </button>
          )}
          {error && <div className="login-err" role="alert">{error}</div>}
        </div>
      </div>
    </div>
  );
}
