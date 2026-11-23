"use client";

import React from "react";
import { api } from "@/lib/api";
import { useLoad } from "@/lib/hooks";
import type { Job, Question, Session, Settings, WritingMode } from "@/lib/types";
import SettingsContent, { WritingModePicker, ZhihuOpenSection } from "./SettingsContent";
import { IconArrowRight, IconCheck, IconCompass, IconInbox, IconQuill, IconSliders, IconSpark, Modal, Spinner } from "./ui";

const allSteps = [
  { key: "model", title: "连接大模型", icon: IconSpark, text: "选择平台和模型，填入 API Key。先测试连接，再保存配置。画像分析与拟稿都需要可用的模型。" },
  { key: "open", title: "配置知乎 Access Secret", icon: IconCompass, text: "这是可选项。启用后使用你自己的知乎开放平台额度；未配置、关闭或请求失败时，助手仍保留原有来源。" },
  { key: "mode", title: "选择写作方式", icon: IconSliders, text: "让助手完成从选题到初稿，或由你把握判断和解答方向。以后也可以在设置里切换。" },
  { key: "profile", title: "分析领域画像", icon: IconSpark, text: "助手会参考你的历史回答与文章，总结擅长的主题和常用写法。没有历史作品也能跳过，稍后手动填写领域。" },
  { key: "sync", title: "同步问题", icon: IconInbox, text: "取回最近三天的邀请和推荐，先排除已答问题。这一步只收题，不自动生成初稿。" },
  { key: "draft", title: "试写第一份初稿", icon: IconQuill, text: "选一道题，写下想保留的判断与理由。助手会查资料并起草，结果留在写作台，不会自动发布。" },
];

export default function Onboarding({ account, canSyncNotifications, onExit }: { account: Session["user"]; canSyncNotifications: boolean; onExit: (path?: string) => void }) {
  const [step, setStep] = React.useState(0);
  const [job, setJob] = React.useState<Job | null>(null);
  const [starting, setStarting] = React.useState(false);
  const [savingMode, setSavingMode] = React.useState(false);
  const [modeChoice, setModeChoice] = React.useState<WritingMode | null>(null);
  const [finishing, setFinishing] = React.useState(false);
  const [error, setError] = React.useState("");
  const [choice, setChoice] = React.useState("");
  const [direction, setDirection] = React.useState("");
  const config = useLoad<Settings>(() => api.get("/api/settings"));
  const inbox = useLoad<{ questions: Question[] }>(() => api.get("/api/questions"), [step, job?.state === "done"]);
  const candidates = (inbox.data?.questions ?? []).filter((question) => question.status === "inbox" && !question.hasDraft);
  const question = candidates.find((item) => item.id === choice) ?? candidates[0];
  const needsDirection = config.data?.writingMode === "subjective" && question?.bucket === "selected"
    && !["imported", "discovered"].includes(question.source);
  const activeQuestion = job?.kind === "research" ? job.questionId : null;
  const steps = canSyncNotifications ? allSteps : allSteps.filter((item) => item.key !== "sync");
  const current = steps[Math.min(step, steps.length - 1)];
  const Icon = current.icon;

  React.useEffect(() => {
    if (job?.state !== "running") return;
    let alive = true;
    const timer = window.setInterval(async () => {
      try {
        const next = await api.get<Job>(`/api/jobs/${job.id}`);
        if (alive) setJob(next);
      } catch {}
    }, 900);
    return () => { alive = false; clearInterval(timer); };
  }, [job?.id, job?.state]);

  async function run(path: string, kind: Job["kind"], body?: object, questionId?: string) {
    setStarting(true);
    setError("");
    try {
      const result = await api.post<{ jobId: string }>(path, body);
      setJob({ id: result.jobId, kind, questionId: questionId ?? null, state: "running", stage: "准备", message: "正在启动…",
        startedAt: new Date().toISOString(), finishedAt: null, progress: 0 });
    } catch (error) {
      setError(error instanceof Error ? error.message : "任务未能启动");
    } finally {
      setStarting(false);
    }
  }

  async function saveMode(writingMode: WritingMode) {
    setModeChoice(writingMode);
    setSavingMode(true);
    setError("");
    try {
      await api.put("/api/settings", { writingMode });
      await config.reload(true);
      window.dispatchEvent(new Event("zhihu-settings-updated"));
    } catch (error) {
      setModeChoice(null);
      setError(error instanceof Error ? error.message : "无法保存模式");
    } finally {
      setSavingMode(false);
    }
  }

  function move(next: number) {
    setStep(next);
    setError("");
    setJob(null);
  }

  async function finish(path?: string) {
    if (finishing) return;
    setFinishing(true);
    try {
      await api.post("/api/session/onboarding/finish");
      onExit(path);
    } catch (error) {
      setError(error instanceof Error ? error.message : "暂时无法保存引导进度");
      setFinishing(false);
    }
  }

  const running = starting || job?.state === "running";
  return <Modal title="欢迎使用知乎答主助手" width={760} dismissible={false} onClose={() => void finish()}>
    <div className="setup-steps" aria-label={`第 ${step + 1} 步，共 ${steps.length} 步`}>
      {steps.map((item, index) => <span key={item.title} title={item.title} data-current={index === step} data-past={index < step} />)}
    </div>
    <section key={step} className="setup-step">
      <div className="setup-heading"><span className="setup-icon"><Icon size={23} /></span><div>
        <small>第 {step + 1} 步 / 共 {steps.length} 步</small><h4>{current.title}</h4>
      </div></div>
      <p className="setup-description">{current.text}</p>
      {current.key === "model" && <SettingsContent embeddedModel />}
      {current.key === "open" && <ZhihuOpenSection account={account} />}
      {current.key === "mode" && (config.data ? <WritingModePicker value={modeChoice ?? config.data.writingMode ?? "automatic"} onChange={saveMode} disabled={savingMode} /> : <Spinner />)}
      {current.key === "profile" && <button className="btn btn-primary" disabled={running || job?.state === "done"} onClick={() => void run("/api/profile/analyze", "profile")}>
        分析我的画像
      </button>}
      {current.key === "sync" && <button className="btn btn-primary" disabled={running || job?.state === "done"} onClick={() => void run("/api/questions/sync/recent", "sync", { selectOnly: true })}>
        同步近 3 天
      </button>}
      {current.key === "draft" && !activeQuestion && (question ? <div className="setup-fields">
        <div className="field"><label htmlFor="setup-question">试写题目</label><select className="select" id="setup-question" value={question.id} disabled={running}
          onChange={(event) => { setChoice(event.target.value); setDirection(""); }}>
          {candidates.map((item) => <option key={item.id} value={item.id}>{item.title}</option>)}
        </select></div>
        <div className="field"><label htmlFor="setup-direction">你的判断与解答方向{needsDirection ? "（必填）" : "（可选）"}</label>
          <textarea id="setup-direction" className="input import-text" rows={3} maxLength={6000} value={direction} disabled={running}
            placeholder="我倾向于……，主要因为……" onChange={(event) => setDirection(event.target.value)} /></div>
        <button className="btn btn-primary" disabled={running || (needsDirection && !direction.trim())} onClick={() => void run(
          `/api/questions/${question.id}/research`, "research", direction.trim() ? { direction } : undefined, question.id)}>生成试写初稿</button>
      </div> : <p className="hint">{inbox.loading ? "正在读取收件箱…" : "收件箱暂无可试写的问题。完成引导后可以导入题目，或主动找题。"}</p>)}
      {job && <div className="setup-job" role="status">
        <strong>{job.state === "failed" ? "未完成" : job.state === "done" ? "已完成" : job.stage}</strong>
        {job.state === "running" && <progress max={1} value={job.progress} />}
        <span>{job.message}</span>
        {job.state === "failed" && activeQuestion && <button className="btn btn-sm" onClick={() => void finish(`/desk?id=${activeQuestion}`)}>去写作台继续</button>}
      </div>}
      {error && <p className="hint" role="alert">{error}</p>}
    </section>
    <div className="setup-footer">
      <button className="btn btn-ghost" disabled={finishing} onClick={() => void finish()}>跳过引导</button>
      <div className="dialog-actions">
        {step > 0 && <button className="btn" disabled={finishing || starting || savingMode} onClick={() => move(step - 1)}>上一步</button>}
        {step < steps.length - 1 ? <button className="btn btn-primary" disabled={finishing || starting || savingMode} onClick={() => move(step + 1)}>
          {running ? "任务继续，进入下一步" : "下一步"} <IconArrowRight size={14} />
        </button> : <button className="btn btn-primary" disabled={finishing} onClick={() => void finish(activeQuestion ? `/desk?id=${activeQuestion}` : "/inbox")}>
          <IconCheck size={14} /> {activeQuestion ? "前往写作台" : "完成引导"}
        </button>}
      </div>
    </div>
  </Modal>;
}
