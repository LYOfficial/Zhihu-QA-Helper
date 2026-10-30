"use client";

import React from "react";
import { api, toast } from "@/lib/api";
import type { Theme } from "@/lib/theme";
import type { Session } from "@/lib/types";
import { IconCheck, IconExternal, Spinner, Seal } from "./ui";

export default function LoginScreen({
  onSuccess,
  theme,
  onThemeChange,
}: {
  onSuccess: () => void;
  theme: Theme;
  onThemeChange: (theme: Theme) => void;
}) {
  /** none = 什么都没开始；paste = 已在默认浏览器打开，等用户粘贴 */
  const [phase, setPhase] = React.useState<"none" | "inapp" | "paste">("none");
  const [browserHint, setBrowserHint] = React.useState("");
  const [cookie, setCookie] = React.useState("");
  const [submitting, setSubmitting] = React.useState(false);
  const [err, setErr] = React.useState("");

  // 应用内登录：开了窗口之后盯会话，成功即进
  React.useEffect(() => {
    if (phase !== "inapp") return;
    const t = setInterval(async () => {
      try {
        const s = await api.get<Session>("/api/session");
        if (s.loggedIn) {
          clearInterval(t);
          toast("登录成功", "ok");
          onSuccess();
        }
      } catch {
        /* 忽略 */
      }
    }, 1500);
    return () => clearInterval(t);
  }, [phase, onSuccess]);

  async function loginInApp() {
    setErr("");
    setBrowserHint("");
    setPhase("inapp");
    try {
      await api.post("/api/login/browser");
      setTimeout(() => {
        setBrowserHint(
          "十秒内没看到登录窗口？说明当前是浏览器预览而不是桌面应用，请改用「用默认浏览器登录」。"
        );
      }, 10_000);
    } catch (e) {
      setPhase("none");
      setErr(e instanceof Error ? e.message : "无法唤起登录窗口");
    }
  }

  async function loginInBrowser() {
    setErr("");
    setBrowserHint("");
    try {
      await api.post("/api/session/open-browser");
      setPhase("paste");
    } catch (e) {
      setErr(e instanceof Error ? e.message : "无法打开浏览器");
    }
  }

  async function submitCookie() {
    setErr("");
    if (cookie.trim().length < 12) {
      setErr("请粘贴完整的 Cookie，至少要包含 z_c0=xxx");
      return;
    }
    setSubmitting(true);
    try {
      await api.post("/api/session/cookie", { cookie });
      toast("登录成功", "ok");
      onSuccess();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Cookie 无效");
    } finally {
      setSubmitting(false);
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
              <b>每天三次自动值守</b>
              定时登录知乎，把邀请与推荐回答取回来，按你的领域归档。
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
          <span>本地运行 · 凭证不出本机</span>
          <span>不采集 · 不上传</span>
        </div>
      </div>

      <div className="login-right">
        <button
          className="login-theme-toggle"
          onClick={() => onThemeChange(theme === "paper" ? "zhihu" : "paper")}
        >
          切换到{theme === "paper" ? "知乎现代蓝" : "宣纸印章黄"}
        </button>
        <div className="login-card">
          <h2>登录知乎</h2>
          <p className="sub">
            邀请与推荐只存在于你的账号通知里。没有登录态，软件无法工作。
          </p>

          <button
            className="btn btn-primary btn-block"
            style={{ marginTop: 18 }}
            onClick={loginInApp}
            disabled={phase === "inapp"}
          >
            {phase === "inapp" ? <Spinner /> : <IconExternal size={14} />}
            {phase === "inapp" ? "等你登录…" : "在应用内登录知乎"}
          </button>
          <p className="hint" style={{ textAlign: "center" }}>
            弹出应用内的知乎登录页，可以在里面扫码或密码登录；登录完自动关闭。
          </p>

          {phase === "inapp" && (
            <div className="login-waiting">
              <Spinner />
              <span>登录窗口已打开，完成登录后会自动回到这里</span>
            </div>
          )}
          {browserHint && <div className="login-warn">{browserHint}</div>}

          <div className="or">
            <span>或</span>
          </div>

          <button className="btn btn-block" onClick={loginInBrowser}>
            <IconExternal size={13} /> 用默认浏览器登录
          </button>
          <p className="hint" style={{ textAlign: "center" }}>
            用你自己的浏览器登录，然后把 z_c0 复制回来粘一次。
          </p>

          {phase === "paste" && (
            <div className="paste-box">
              <div className="steps">
                <b>复制 z_c0 的步骤</b>
                <ol>
                  <li>在刚打开的浏览器里完成知乎登录（扫码或账号密码都行）</li>
                  <li>按 <kbd>F12</kbd> → <b>Application</b> → <b>Cookies</b> → https://www.zhihu.com</li>
                  <li>找到 <b>z_c0</b>，双击 Value 全选复制</li>
                  <li>粘贴到下面这个框里</li>
                </ol>
              </div>
              <textarea
                className="textarea"
                style={{ minHeight: 74, fontSize: 11.5, fontFamily: "var(--font-num)" }}
                placeholder="z_c0=xxxxxxxxxxxx  （粘贴整行 Cookie 也可以）"
                value={cookie}
                onChange={(e) => setCookie(e.target.value)}
              />
              <button
                className="btn btn-primary btn-block"
                style={{ marginTop: 8 }}
                onClick={submitCookie}
                disabled={submitting}
              >
                {submitting ? <Spinner /> : <IconCheck size={13} />} 用这段 Cookie 登录
              </button>
            </div>
          )}

          {err && <div className="login-err">{err}</div>}

          <div className="login-steps">
            <b>为什么默认浏览器登录还得多一步复制？</b>
            <br />
            知乎没有提供回调地址，浏览器里的登录态属于浏览器，软件读不到。
            「在应用内登录」用的是软件自己的浏览器内核，所以能自动取到，不需要复制。
          </div>
        </div>
      </div>
    </div>
  );
}
