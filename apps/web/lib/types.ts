/** 与 crates/core/src/models.rs 一一对应（serde camelCase 输出） */

export type QuestionStatus =
  | "inbox"
  | "researching"
  | "pending_review"
  | "answered"
  | "dismissed";

export type Bucket = "queue" | "selected" | "archived";

export interface ZhihuUser {
  id: string;
  name: string;
  urlToken: string;
  avatarUrl: string;
  headline: string;
  answerCount: number;
  articlesCount: number;
  followerCount: number;
}

export interface Session {
  loggedIn: boolean;
  user: ZhihuUser | null;
  expiresAt: string | null;
  lastSyncAt: string | null;
  signMode: string;
}

export interface QrTicket {
  token: string;
  link: string;
  svg: string;
  expiresAt: number;
}

export interface QrState {
  status: "waiting" | "scanned" | "confirmed" | "expired" | "error";
  user: ZhihuUser | null;
  message: string;
}

export interface ProfileTerm {
  id: string;
  kind: "primary" | "sub" | "adjacent";
  label: string;
  weight: number;
  keywords: string[];
  locked: boolean;
  note: string;
}

export interface Profile {
  status: "empty" | "running" | "ready";
  summary: string;
  updatedAt: string | null;
  terms: ProfileTerm[];
  evidence: string[];
  sampleSize: number;
}

export interface Source {
  id: string;
  domain: string;
  name: string;
  baseUrl: string;
  kind: "wiki" | "community" | "official" | "media" | "dataset" | "forum";
  trust: number;
  enabled: boolean;
  notes: string;
  useFor: string[];
  discoveredBy: string;
  ok: boolean;
}

export interface Question {
  id: string;
  zhihuId: string;
  questionId: string;
  source: "invite" | "recommend";
  title: string;
  excerpt: string;
  url: string;
  domain: string;
  subDomain: string;
  fit: number;
  status: QuestionStatus;
  bucket: Bucket;
  answerCount: number;
  followerCount: number;
  askerName: string;
  askerHeadline: string;
  tags: string[];
  createdAt: string;
  syncedAt: string;
  hasDraft: boolean;
  imageCount: number;
  wordCount: number;
  doubtCount: number;
}

export interface Citation {
  label: string;
  url: string;
  sourceName: string;
  snippet: string;
}

export interface Doubt {
  text: string;
  status: "open" | "verified" | "corrected" | "rejected";
  verdict: string;
  source: string;
}

export interface ImageAsset {
  id: string;
  questionId: string;
  filename: string;
  url: string;
  sourceUrl: string;
  sourceName: string;
  caption: string;
  bytes: number;
  createdAt: string;
}

export interface Draft {
  questionId: string;
  markdown: string;
  doubts: Doubt[];
  citations: Citation[];
  images: ImageAsset[];
  /** 拟稿过程中的提示：没抓到资料、配图不足、输出被截断等 */
  warnings?: string[];
  wordCount: number;
  updatedAt: string;
  model: string;
}

export interface ModelConfig {
  provider: string;
  baseUrl: string;
  model: string;
  apiKey: string;
  hasKey: boolean;
  temperature: number;
  verifiedAt: string | null;
  /** 上下文窗口，来自 models.dev 的 limit.context；0 表示未知 */
  contextLimit: number;
  /** 单次最大输出，来自 models.dev 的 limit.output；0 表示未知 */
  outputLimit: number;
}

export interface Settings {
  cronTimes: string[];
  pickRatio: number;
  draftMin: number;
  draftMax: number;
  imageMin: number;
  imageMax: number;
  autoSync: boolean;
  minimizeToTray: boolean;
  autoLaunch: boolean;
  signMode: string;
  dataDir: string;
}

export interface Job {
  id: string;
  kind: "sync" | "research" | "profile" | "sources";
  questionId: string | null;
  stage: string;
  progress: number;
  message: string;
  state: "running" | "done" | "failed";
  startedAt: string;
  finishedAt: string | null;
}

export interface EventItem {
  id: number;
  kind: string;
  questionId: string | null;
  title: string;
  detail: string;
  createdAt: string;
}

export interface StatsOverview {
  totalQuestions: number;
  inbox: number;
  pendingReview: number;
  answered: number;
  dismissed: number;
  drafts: number;
  images: number;
  imagesBytes: number;
  answers30d: number;
  voteups30d: number;
  newComments30d: number;
  avgFit: number;
  hitRate: number;
  savedMinutes: number;
}

export interface StatsPoint {
  date: string;
  collected: number;
  answered: number;
  voteups: number;
  comments: number;
}

export interface Health {
  ok: boolean;
  version: string;
  agent: boolean;
  db: string;
  dataDir: string;
}

export interface DomainCount {
  domain: string;
  count: number;
}
