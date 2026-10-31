"use client";

import React from "react";
import {
  IconCheck,
  IconRefresh,
  IconUser,
  Modal,
  SectionTitle,
  Spinner,
  Toggle,
  relTime,
} from "@/components/ui";
import { api, getMode, openExternal, toast } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import { PROVIDERS } from "@/lib/providers";
import type { Health, ModelConfig, QrTicket, Session, Settings } from "@/lib/types";

/** 把 1000000 写成 1M，界面上不占地方又看得懂 */
function formatTokens(n: number): string {
  if (!n) return "未知";
  if (n >= 1_000_000) {
    const v = n / 1_000_000;
    return `${Number.isInteger(v) ? v : v.toFixed(1)}M`;
  }
  if (n >= 1000) return `${Math.round(n / 1000)}K`;
  return String(n);
}

export default function SettingsPage() {
  const se = useLoad<Settings>(() => api.get("/api/settings"));
  const mo = useLoad<ModelConfig>(() => api.get("/api/model"));
  const ss = useLoad<Session>(() => api.get("/api/session"));
  const he = useLoad<Health>(() => api.get("/api/health"));

  const [settings, setSettings] = React.useState<Settings | null>(null);
  const [model, setModel] = React.useState<ModelConfig | null>(null);
  const [test, setTest] = React.useState<{ ok: boolean; msg: string } | null>(null);
  const [models, setModels] = React.useState<string[]>([]);
  const [catalogModels, setCatalogModels] = React.useState<Record<string, string[]>>({});
  const [catalogLimits, setCatalogLimits] = React.useState<
    Record<string, Record<string, { context: number; output: number }>>
  >({});
  const [catalogSyncing, setCatalogSyncing] = React.useState(false);
  const [catalogAt, setCatalogAt] = React.useState("");
  const [testing, setTesting] = React.useState(false);
  const [savingModel, setSavingModel] = React.useState(false);

  function sortCronTimes(times: string[]) {
    return [...times].sort((a, b) => a.localeCompare(b));
  }

  function nextCronTime(times: string[]) {
    const used = new Set(times);
    for (let minute = 0; minute < 24 * 60; minute += 30) {
      const value = `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
      if (!used.has(value)) return value;
    }
    return "22:00";
  }

  React.useEffect(() => {
    if (se.data) setSettings(se.data);
  }, [se.data]);
  React.useEffect(() => {
    if (mo.data) setModel(mo.data);
  }, [mo.data]);

  async function saveSettings(patch: Partial<Settings>) {
    if (!settings) return;
    const next = {
      ...settings,
      ...patch,
      cronTimes: patch.cronTimes ? sortCronTimes(patch.cronTimes) : settings.cronTimes,
    };
    setSettings(next);
    try {
      const saved = await api.put<Settings>("/api/settings", next);
      setSettings(saved);
      window.dispatchEvent(new Event("zhihu-settings-updated"));
    } catch (e) {
      toast(e instanceof Error ? e.message : "保存失败", "err");
    }
  }

  async function saveModel() {
    if (!model) return;
    if (!model.provider.trim() || !model.model.trim()) {
      toast("请先选择模型平台并填写模型名", "err");
      return;
    }
    try {
      const url = new URL(model.baseUrl);
      if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error();
    } catch {
      toast("接口地址必须是 http:// 或 https:// 开头", "err");
      return;
    }
    setSavingModel(true);
    try {
      const saved = await api.put<ModelConfig>("/api/model", model);
      setModel(saved);
      toast("模型配置已保存", "ok");
    } catch (e) {
      toast(e instanceof Error ? e.message : "保存失败", "err");
    } finally {
      setSavingModel(false);
    }
  }

  async function testModel() {
    if (!model) return;
    if (!model.provider.trim() || !model.model.trim() || !model.baseUrl.trim()) {
      setTest({ ok: false, msg: "请先填写平台、接口地址和模型名" });
      return;
    }
    setTesting(true);
    setTest(null);
    try {
      const r = await api.post<{ ok: boolean; message: string; latencyMs: number }>(
        "/api/model/test",
        model
      );
      setTest({ ok: r.ok, msg: `${r.message}` });
    } catch (e) {
      setTest({ ok: false, msg: e instanceof Error ? e.message : "测试失败" });
    } finally {
      setTesting(false);
    }
  }

  // 注意：preset 与这个 effect 必须在下面的条件 return 之前，
  // 否则 hooks 顺序在两次渲染间会变，直接抛 client-side exception。
  const preset = PROVIDERS.find((p) => p.key === model?.provider) || PROVIDERS[0];
  React.useEffect(() => {
    setModels(catalogModels[preset.key] ?? preset.models);
  }, [preset.key, catalogModels]);

  async function syncCatalog(silent = false) {
    setCatalogSyncing(true);
    try {
      const r = await api.get<{
        providers?: Record<string, string[]>;
        limits?: Record<string, Record<string, { context: number; output: number }>>;
      }>("/api/model/catalog");
      const nextCatalog = r.providers ?? {};
      setCatalogModels(nextCatalog);
      setCatalogLimits(r.limits ?? {});
      const current = nextCatalog[model?.provider ?? ""] ?? [];
      if (current.length) setModels(current);
      // 顺手把当前模型的容量补齐：之前存的是 0，说明还没同步过
      const lim = r.limits?.[model?.provider ?? ""]?.[model?.model ?? ""];
      if (lim && model && (!model.contextLimit || !model.outputLimit)) {
        setModel({ ...model, contextLimit: lim.context, outputLimit: lim.output });
      }
      setCatalogAt(new Date().toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" }));
      window.sessionStorage.setItem("zhihu-qa-helper-model-catalog-synced", "1");
      const withLimit = Object.values(r.limits ?? {}).reduce(
        (n, byModel) => n + Object.keys(byModel).length,
        0
      );
      if (!silent) {
        toast(
          current.length
            ? `模型库已同步：当前平台 ${current.length} 个模型，${withLimit} 个带上下文容量`
            : "模型库已同步",
          "ok"
        );
      }
    } catch (e) {
      if (!silent) toast(e instanceof Error ? e.message : "模型库同步失败，继续使用离线清单", "err");
    } finally {
      setCatalogSyncing(false);
    }
  }

  const currentLimit = catalogLimits[model?.provider ?? ""]?.[model?.model ?? ""];
  const ctxLimit = currentLimit?.context || model?.contextLimit || 0;
  const outLimit = currentLimit?.output || model?.outputLimit || 0;

  // 每次启动静默同步一次模型库：把 models.dev 的上下文容量补到当前模型上，
  // 拟稿时才知道能一次性喂多少资料。失败就继续用内置清单，不打扰用户。
  const autoSynced = React.useRef(false);
  React.useEffect(() => {
    if (autoSynced.current || !model) return;
    autoSynced.current = true;
    if (catalogAt) return;
    if (window.sessionStorage.getItem("zhihu-qa-helper-model-catalog-synced") === "1") return;
    void syncCatalog(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [model]);

  if (!settings || !model) {
    return (
      <main className="page">
        <div style={{ padding: 40, color: "var(--ink-3)", fontSize: 13 }}>
          <Spinner /> 正在读取配置…
        </div>
      </main>
    );
  }

  return (
    <main className="page">
      <header className="page-head">
        <div>
          <h1 className="page-title">
            设置 <small>SETUP</small>
          </h1>
          <p className="page-sub">账号、模型、值守节奏与桌面行为。所有配置都存在本机。</p>
        </div>
      </header>

      <div className="page-body">

      <div className="split">
        <div>
          {/* 账号 */}
          <SectionTitle
            right={
              <span className="dim">
                {ss.data?.loggedIn ? "已登录 · 凭证存于本机" : "扫码登录"}
              </span>
            }
          >
            知乎账号
          </SectionTitle>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 14,
              padding: "14px 16px",
              border: "1px solid var(--rule)",
              background: "var(--paper-raised)",
              marginBottom: 12,
            }}
          >
            <div
              style={{
                width: 42,
                height: 42,
                borderRadius: "50%",
                background: "var(--paper-sunken)",
                display: "grid",
                placeItems: "center",
                fontFamily: "var(--font-kai)",
                fontSize: 17,
                color: "var(--ink-3)",
                overflow: "hidden",
                flexShrink: 0,
              }}
            >
              {ss.data?.user?.avatarUrl ? (
                // eslint-disable-next-line @next/next/no-img-element
                <img
                  src={ss.data.user.avatarUrl}
                  alt=""
                  style={{ width: "100%", height: "100%", objectFit: "cover" }}
                />
              ) : (
                <IconUser size={19} />
              )}
            </div>
            <div style={{ minWidth: 0, flex: 1 }}>
              <div style={{ fontSize: 14 }}>
                {ss.data?.loggedIn ? ss.data.user?.name : "未登录"}
              </div>
              <div style={{ fontSize: 11.5, color: "var(--ink-3)" }}>
                {ss.data?.loggedIn
                  ? `${ss.data.user?.answerCount ?? 0} 回答 · ${ss.data.user?.articlesCount ?? 0} 文章 · 上次同步 ${relTime(ss.data.lastSyncAt)}`
                  : "登录后才会拉取你的邀请与推荐回答"}
              </div>
            </div>
            {ss.data?.loggedIn ? (
              <div style={{ display: "flex", gap: 6 }}>
                <button
                  className="btn btn-sm"
                  onClick={async () => {
                    try {
                      await api.post("/api/session/refresh");
                      toast("账号资料已刷新", "ok");
                      ss.reload(true);
                    } catch (e) {
                      toast(e instanceof Error ? e.message : "失败", "err");
                    }
                  }}
                >
                  <IconRefresh size={12} /> 刷新
                </button>
                <button
                  className="btn btn-sm"
                  onClick={async () => {
                    await api.post("/api/session/logout");
                    toast("已退出登录", "ok");
                    setTimeout(() => window.location.reload(), 350);
                  }}
                >
                  退出
                </button>
              </div>
            ) : (
              <span className="hint" style={{ margin: 0 }}>
                未登录
              </span>
            )}
          </div>

          {/* 模型 */}
          <div className="mt-24">
            <SectionTitle right={<span className="dim">首次启动同步</span>}>大模型</SectionTitle>
          </div>

          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
              gap: 7,
              marginBottom: 16,
            }}
          >
            {PROVIDERS.map((p) => (
              <button
                key={p.key}
                className="btn btn-sm"
                onClick={() =>
                  setModel({
                    ...model,
                    provider: p.key,
                    baseUrl: p.baseUrl,
                    model: p.models[0],
                    contextLimit: catalogLimits[p.key]?.[p.models[0]]?.context ?? 0,
                    outputLimit: catalogLimits[p.key]?.[p.models[0]]?.output ?? 0,
                  })
                }
                style={{
                  justifyContent: "center",
                  borderColor: model.provider === p.key ? "var(--blue)" : "var(--rule-2)",
                  background:
                    model.provider === p.key ? "var(--blue-wash)" : "rgba(251,249,244,.6)",
                  color: model.provider === p.key ? "var(--blue)" : "var(--ink-2)",
                }}
              >
                {p.label}
              </button>
            ))}
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 14 }}>
            <button className="btn btn-sm" onClick={() => syncCatalog()} disabled={catalogSyncing}>
              {catalogSyncing ? <Spinner /> : <IconRefresh size={13} />} 同步模型库
            </button>
            <span className="hint" style={{ margin: 0 }}>
              {catalogAt
                ? `最近同步 ${catalogAt} · models.dev 开源目录`
                : "网络不可用时继续使用内置兜底清单"}
            </span>
          </div>

          <div className="field">
            <label>接口地址 Base URL</label>
            <input
              className="input"
              value={model.baseUrl}
              onChange={(e) => setModel({ ...model, baseUrl: e.target.value })}
              placeholder={preset.baseUrl}
            />
          </div>

          <div className="row">
            <div className="field">
              <label>模型名</label>
              <div style={{ display: "flex", gap: 6 }}>
                <input
                  className="input"
                  list="model-list"
                  value={model.model}
                  onChange={(e) => {
                    const name = e.target.value;
                    const lim = catalogLimits[model.provider]?.[name];
                    setModel({
                      ...model,
                      model: name,
                      ...(lim ? { contextLimit: lim.context, outputLimit: lim.output } : {}),
                    });
                  }}
                />
                <button className="btn btn-sm" style={{ flexShrink: 0 }} onClick={() => syncCatalog()} disabled={catalogSyncing}>
                  {catalogSyncing ? <Spinner /> : "同步"}
                </button>
              </div>
              <datalist id="model-list">
                {models.map((m) => {
                  const lim = catalogLimits[model.provider]?.[m];
                  return (
                    <option key={m} value={m}>
                      {lim ? `上下文 ${formatTokens(lim.context)} · 输出 ${formatTokens(lim.output)}` : ""}
                    </option>
                  );
                })}
              </datalist>
              <p className="hint" style={{ marginTop: 5, marginBottom: 0, lineHeight: 1.6 }}>
                {ctxLimit > 0
                  ? `上下文 ${formatTokens(ctxLimit)} tokens${outLimit > 0 ? ` · 单次输出上限 ${formatTokens(outLimit)}` : ""}`
                  : "上下文容量未知：点「同步模型库」可自动补全，拟稿时据此决定一次性喂多少资料"}
              </p>
            </div>
            <div className="field">
              <label>API Key</label>
              <input
                className="input"
                type="password"
                value={model.apiKey}
                onChange={(e) => setModel({ ...model, apiKey: e.target.value })}
                placeholder={preset.keyHint}
              />
            </div>
          </div>

          <p className="hint" style={{ marginTop: -6, marginBottom: 14, lineHeight: 1.75 }}>
            模型清单来自 models.dev 开源目录。首次启动时会自动同步，之后只在这里手动点击“同步模型库”时更新。
            {preset.docs && (
              <>
                {" "}
                <a
                  href={preset.docs}
                  onClick={(e) => {
                    e.preventDefault();
                    void openExternal(preset.docs);
                  }}
                >
                  查看 {preset.label} 文档 ↗
                </a>
              </>
            )}
            <br />
            已保存的 Key 以掩码显示，留空表示不修改。
          </p>

          <div className="field">
            <label>温度 {model.temperature.toFixed(2)}</label>
            <input
              className="range"
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={model.temperature}
              onChange={(e) => setModel({ ...model, temperature: Number(e.target.value) })}
            />
            <div className="hint">0.3 偏严谨，0.6 以上更像人写的。拟稿建议 0.4–0.5。</div>
          </div>

          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <button className="btn btn-primary" onClick={saveModel} disabled={savingModel}>
              {savingModel ? <Spinner /> : <IconCheck size={13} />} 保存配置
            </button>
            <button className="btn" onClick={testModel} disabled={testing}>
              {testing ? <Spinner /> : null} 测试连接
            </button>
            {test && (
              <span
                style={{
                  fontSize: 12,
                  color: test.ok ? "var(--moss)" : "var(--cinnabar)",
                  lineHeight: 1.6,
                }}
              >
                {test.ok ? "✓ " : "✗ "}
                {test.msg}
              </span>
            )}
          </div>

          {!he.data?.agent && (
            <p className="hint" style={{ color: "var(--amber)", marginTop: 8 }}>
              本地智能体（Python 服务）没有运行，测试与拟稿都会失败。重启软件即可自动拉起。
            </p>
          )}

          {/* 节奏 */}
          <div className="mt-24">
            <SectionTitle right={<span className="dim">每天 {settings.cronTimes.length} 次</span>}>值守与拟稿节奏</SectionTitle>
          </div>

          <div className="field">
            <label>自动登录时间</label>
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              {settings.cronTimes.map((t, i) => (
                <input
                  key={i}
                  className="input mono"
                  style={{ width: 88 }}
                  value={t}
                  onChange={(e) => {
                    const next = [...settings.cronTimes];
                    next[i] = e.target.value;
                    setSettings({ ...settings, cronTimes: next });
                  }}
                  onBlur={() => {
                    void saveSettings({ cronTimes: settings.cronTimes });
                  }}
                />
              ))}
              <button
                className="btn btn-sm"
                onClick={() => saveSettings({ cronTimes: [...settings.cronTimes, nextCronTime(settings.cronTimes)] })}
              >
                + 加一次
              </button>
            </div>
            <div className="hint">
              可添加任意多个时间点，保存后会自动按先后顺序排列；知乎每次登录并打开通知就会生成新的邀请提醒。
            </div>
          </div>

          <div className="field">
            <label>重点队列比例 {(settings.pickRatio * 100).toFixed(0)}%</label>
            <input
              className="range"
              type="range"
              min={0.1}
              max={0.9}
              step={0.05}
              value={settings.pickRatio}
              onChange={(e) => saveSettings({ pickRatio: Number(e.target.value) })}
            />
            <div className="hint">
              每次同步后，按契合度取前 {((settings.pickRatio || 0.4) * 100).toFixed(0)}% 的问题自动拟稿。
            </div>
          </div>

          <div className="row">
            <div className="field">
              <label>
                初稿字数 {settings.draftMin} – {settings.draftMax}
              </label>
              <input
                className="range"
                type="range"
                min={300}
                max={2000}
                step={50}
                value={settings.draftMax}
                onChange={(e) =>
                  saveSettings({
                    draftMax: Number(e.target.value),
                    draftMin: Math.min(settings.draftMin, Number(e.target.value) - 100),
                  })
                }
              />
            </div>
            <div className="field">
              <label>
                配图张数 {settings.imageMin} – {settings.imageMax}
              </label>
              <input
                className="range"
                type="range"
                min={1}
                max={12}
                step={1}
                value={settings.imageMax}
                onChange={(e) =>
                  saveSettings({
                    imageMax: Number(e.target.value),
                    imageMin: Math.min(settings.imageMin, Number(e.target.value)),
                  })
                }
              />
            </div>
          </div>

          {/* 桌面 */}
          <div className="mt-24">
            <SectionTitle>桌面行为</SectionTitle>
          </div>
          <div className="switch">
            <div>
              <div className="switch-label">到点自动同步</div>
              <div className="switch-desc">关闭后只能手动点「同步通知」</div>
            </div>
            <Toggle
              on={settings.autoSync}
              onChange={(v) => saveSettings({ autoSync: v })}
            />
          </div>
          <div className="switch">
            <div>
              <div className="switch-label">关闭窗口时最小化到托盘</div>
              <div className="switch-desc">保持后台值守，右键托盘图标可退出</div>
            </div>
            <Toggle
              on={settings.minimizeToTray}
              onChange={(v) => saveSettings({ minimizeToTray: v })}
            />
          </div>

          <div className="field mt-16">
            <label>知乎请求签名策略</label>
            <select
              className="select"
              value={settings.signMode}
              onChange={(e) => saveSettings({ signMode: e.target.value })}
            >
              <option value="auto">自动（签名失败时降级重试）</option>
              <option value="always">始终签名</option>
              <option value="off">不签名（仅 Cookie）</option>
            </select>
            <div className="hint">
              知乎的风控参数会随版本变化。若同步报 403，先切到「仅 Cookie」试试；
              这是接口层面的兼容开关，不影响账号安全。
            </div>
          </div>
        </div>

        {/* 右侧 */}
        <aside>
          <SectionTitle>本机状态</SectionTitle>
          <div className="kv">
            <span>内核版本</span>
            <b>{he.data?.version ?? "—"}</b>
          </div>
          <div className="kv">
            <span>数据库</span>
            <b>{he.data?.db ?? "—"}</b>
          </div>
          <div className="kv">
            <span>智能体</span>
            <b style={{ color: he.data?.agent ? "var(--moss)" : "var(--cinnabar)" }}>
              {he.data?.agent ? "运行中" : "未运行"}
            </b>
          </div>
          <div className="kv">
            <span>数据目录</span>
            <b
              style={{
                fontSize: 10.5,
                maxWidth: 170,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
              title={he.data?.dataDir}
            >
              {he.data?.dataDir ?? "—"}
            </b>
          </div>

          <div className="divider" />

          <SectionTitle>隐私</SectionTitle>
          <p style={{ fontSize: 11.5, color: "var(--ink-3)", lineHeight: 1.8 }}>
            知乎 Cookie 只写在本地 SQLite；拟稿时只把你选择的问题、画像与抓到的公开资料发给模型，
            不会上传 Cookie、历史回答原文或任何账号凭证。
          </p>

          <div className="divider" />

          <SectionTitle>数据</SectionTitle>
          <div className="kv">
            <span>数据库</span>
            <b style={{ fontSize: 10.5 }}>zhihu-qa.db</b>
          </div>
          <div className="kv">
            <span>归档正文</span>
            <b style={{ fontSize: 10.5 }}>data/domains/</b>
          </div>
          <div className="kv">
            <span>配图缓存</span>
            <b style={{ fontSize: 10.5 }}>data/images/</b>
          </div>
          <p className="hint" style={{ marginTop: 8 }}>
            已回答或弃用的问题会清掉本地配图，只保留归档的正文。
          </p>
        </aside>
      </div>


    </div>
    </main>
  );
}
