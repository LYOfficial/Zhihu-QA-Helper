"use client";

import { api, toast } from "./api";
import type { Job } from "./types";

/**
 * 跟着一个后台 job 走，把进度和失败原因如实抛给界面。
 * 后台任务失败时如果不提示，用户只会看到问题莫名其妙退回「待阅」——
 * 那等于把错误藏起来，违反这个工具的基本原则。
 */
export function watchJob(
  questionId: string,
  kind: Job["kind"],
  onFinish: () => void
): () => void {
  let tries = 0;
  let lastStage = "";
  const timer = setInterval(async () => {
    tries += 1;
    try {
      const { jobs } = await api.get<{ jobs: Job[] }>("/api/jobs");
      const job = jobs.find((j) => j.questionId === questionId && j.kind === kind);
      if (job) {
        if (job.state === "failed") {
          clearInterval(timer);
          toast(
            kind === "research" ? `拟稿失败：${job.message}` : `任务失败：${job.message}`,
            "err"
          );
          onFinish();
          return;
        }
        if (job.state === "done") {
          clearInterval(timer);
          toast(kind === "research" ? "初稿已就绪，可以去批阅了" : "任务完成", "ok");
          onFinish();
          return;
        }
        if (job.stage && job.stage !== lastStage) {
          lastStage = job.stage;
        }
      }
    } catch {
      /* 单次轮询失败忽略 */
    }
    if (tries > 300) {
      // 10 分钟还没结束：明确告诉用户，不要留下一个永远转圈的界面
      clearInterval(timer);
      toast("这个任务比预期久，已停止跟随。可以点「刷新状态」查看结果", "info");
    }
  }, 2000);
  return () => clearInterval(timer);
}
