// Bilingual term definitions — the glossary the onboarding issue asks for
// (issue #82 P1-8). Lives in its own module so the I18n context file only
// exports components/hooks (react-refresh constraint).

export const GLOSSARY: Record<string, { en: string; zh: string }> = {
  pipeline: {
    en: 'A reusable bioinformatics workflow: a sequence of rules connected by file dependencies.',
    zh: '可复用的生信工作流：由文件依赖串起来的一系列规则。',
  },
  rule: {
    en: 'One analysis step: a shell command with declared inputs and outputs.',
    zh: '一个分析步骤：带输入输出声明的 shell 命令。',
  },
  wildcard: {
    en: 'A placeholder like {sample} that expands into real values for each sample.',
    zh: '如 {sample} 的占位符，会展开成每个样本的实际值。',
  },
  checkpoint: {
    en: 'The engine’s record of completed rules — makes re-runs resume instead of restart.',
    zh: '引擎对已完成规则的记录 — 让重跑变成续跑而不是从头再来。',
  },
  'dry-run': {
    en: 'A read-only preview of what a run would execute, without running anything.',
    zh: '只读预览：展示一次运行会执行什么，但什么都不真正执行。',
  },
  depends_on: {
    en: 'Declares rule ordering: this rule waits for the named rules to finish.',
    zh: '声明规则顺序：此规则等待指定规则完成后才执行。',
  },
  workdir: {
    en: 'The directory a run executes in — logs, checkpoint state and results live here.',
    zh: '运行执行的目录 — 日志、checkpoint 状态与结果都存放在这里。',
  },
};
