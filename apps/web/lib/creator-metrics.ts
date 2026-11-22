import type { AnswerMetrics, QuestionMetrics } from "./types";

const numberFormat = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 1 });

export function formatMetric(value: number | null) {
  return value === null ? "—" : numberFormat.format(value);
}

export function answerQuality(metrics: AnswerMetrics) {
  const { reads, votes, comments, favorites, likes } = metrics;
  if (reads === null || votes === null || comments === null || favorites === null || likes === null) return null;
  return 1000 * (votes + 2 * comments + 1.5 * favorites + 0.5 * likes) / (reads + 1000);
}

export function qualityLevel(score: number) {
  if (score >= 80) return { label: "极优", tone: "extreme" };
  if (score >= 40) return { label: "高质", tone: "high" };
  if (score >= 15) return { label: "中等", tone: "mid" };
  return { label: "待观察", tone: "low" };
}

export function questionOpportunity(metrics: QuestionMetrics | null, now = Date.now()) {
  if (!metrics) return null;
  const { views, answers, followers, createdTime } = metrics;
  if (views === null || answers === null || followers === null) return null;
  const ageDays = createdTime === null ? 180 : Math.max(0, (now / 1000 - createdTime) / 86400);
  return 100 * Math.log10(views + 10) * Math.log10(followers + 10)
    / Math.sqrt((answers + 3) * (1 + ageDays / 365)) * (views / (views + 100000));
}

export function opportunityLevel(score: number) {
  if (score >= 120) return { label: "极高机会", tone: "extreme" };
  if (score >= 75) return { label: "高机会", tone: "high" };
  if (score >= 40) return { label: "中机会", tone: "mid" };
  return { label: "低机会", tone: "low" };
}
