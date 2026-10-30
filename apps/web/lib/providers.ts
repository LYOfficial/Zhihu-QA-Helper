export interface ProviderPreset {
  key: string;
  label: string;
  baseUrl: string;
  models: string[];
  keyHint: string;
  docs: string;
}

/**
 * 与 agents/llm.py 的 PROVIDERS 保持一致。
 *
 * 注意各家原生模型 ID 的写法差异（写错就是连不上）：
 *   - Anthropic 原生：claude-sonnet-5-5（短横线）
 * 内置清单是离线兜底；设置页会从 models.dev 开源目录自动同步最新模型。
 */
export const PROVIDERS: ProviderPreset[] = [
  {
    key: "openai",
    label: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    models: ["gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-astra", "gpt-6-luna", "gpt-5.6-sol"],
    keyHint: "sk-...",
    docs: "https://platform.openai.com/docs/models",
  },
  {
    key: "anthropic",
    label: "Claude",
    baseUrl: "https://api.anthropic.com/v1",
    models: [
      "claude-opus-5-5",
      "claude-sonnet-5-5",
      "claude-haiku-5-5",
      "claude-fable-5-1",
      "claude-mythos-5-1",
    ],
    keyHint: "sk-ant-...",
    docs: "https://platform.claude.com/docs/en/docs/about-claude/models",
  },
  {
    key: "deepseek",
    label: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    models: ["deepseek-flash", "deepseek-v4-pro"],
    keyHint: "sk-...",
    docs: "https://api-docs.deepseek.com/quick_start/pricing",
  },
  {
    key: "glm",
    label: "智谱 GLM",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    models: ["glm-5", "glm-4.7", "glm-4.6", "glm-4-flash"],
    keyHint: "xxxx.xxxx",
    docs: "https://docs.bigmodel.cn",
  },
  {
    key: "kimi",
    label: "Kimi",
    baseUrl: "https://api.moonshot.cn/v1",
    models: ["kimi-k3.1-pro", "kimi-k3.1-pro-highspeed", "kimi-k3", "kimi-k2.6"],
    keyHint: "sk-...",
    docs: "https://platform.kimi.com/docs/get-api-key",
  },
  {
    key: "qwen",
    label: "通义千问",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    models: ["qwen3.8-max", "qwen3.7-plus", "qwen3.8-flash", "qwen3.8-omni-flash"],
    keyHint: "sk-...",
    docs: "https://help.aliyun.com/zh/model-studio/models",
  },
  {
    key: "minimax",
    label: "MiniMax",
    baseUrl: "https://api.minimax.chat/v1",
    models: ["MiniMax-M3.1", "MiniMax-M3.1-Flash", "MiniMax-M3"],
    keyHint: "eyJ...",
    docs: "https://platform.minimaxi.com",
  },
  {
    key: "custom",
    label: "自定义",
    baseUrl: "http://127.0.0.1:11434/v1",
    models: ["qwen2.5:14b"],
    keyHint: "本地可留空",
    docs: "",
  },
];

export const providerOf = (key: string) =>
  PROVIDERS.find((p) => p.key === key) ?? PROVIDERS[0];
