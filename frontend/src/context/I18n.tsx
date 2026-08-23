/* eslint-disable react-refresh/only-export-components */
// Lightweight chrome-level i18n (issue #82 P1-8): the interface chrome
// (navigation, dashboard, common actions) switches between English and
// Simplified Chinese. Domain content (workflow TOML, tool names) stays in
// English — it is the lingua franca of bioinformatics pipelines.

import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

export type Lang = 'en' | 'zh';

export function getLocale(lang: Lang): string {
  return lang === 'zh' ? 'zh-CN' : 'en-US';
}

const TRANSLATIONS: Record<Lang, Record<string, string>> = {
  en: {
    'nav.dashboard': 'Dashboard',
    'nav.editor': 'Pipeline Editor',
    'nav.pipelines': 'Pipelines',
    'nav.runs': 'Runs',
    'nav.chat': 'AI Chat',
    'nav.docs': 'API Docs',
    'nav.clusters': 'Clusters',
    'nav.users': 'Users',
    'nav.audit': 'Audit',
    'nav.settings': 'Settings',
    'nav.guest': 'Guest — sign in',
    'nav.signedIn': 'Signed in',
    'dashboard.title': 'What do you want to do?',
    'dashboard.subtitle': 'Design, run, and share bioinformatics pipelines — with AI assistance at every step.',
    'dashboard.ai': 'Generate with AI',
    'dashboard.ai.desc': 'Describe your analysis in plain language and let the assistant draft the pipeline.',
    'dashboard.ai.disabledNote': 'AI assistant is not set up',
    'dashboard.ai.disabledDesc': 'Enable an AI provider in Settings to generate pipelines from a description.',
    'dashboard.ai.disabledCta': 'Start from a template',
    'dashboard.templates': 'Start from a template',
    'dashboard.templates.desc': 'Battle-tested workflows for RNA-seq, WGS, scRNA, QIIME2 and more.',
    'dashboard.runs': 'Recent runs',
    'dashboard.noRuns': 'No runs yet — your finished analyses will appear here.',
    'dashboard.create': 'Open the editor',
    'dashboard.onboarding': 'Getting started',
    'dashboard.onboarding.steps': [
      'Create a pipeline with the AI assistant or a template',
      'Preview the plan with Dry-Run, then execute',
      'Inspect results, download files, and share the pipeline',
    ].join('|'),
    'dashboard.onboarding.dismiss': 'Got it',
    'chat.disabled.message': 'AI assistant is not set up on this server. You can build a pipeline from a template or write it step by step.',
    'chat.disabled.templates': 'Browse templates',
    'chat.disabled.editor': 'Open editor',
    'run.title': 'Run options',
    'run.parallelJobs': 'Parallel jobs (max)',
    'run.samples': 'Samples — run only this subset (names, first:N, ready; empty = all)',
    'run.targets': 'Target rules (comma-separated; empty = engine default)',
    'run.keepGoing': 'Keep going when a rule fails',
    'run.dryRun': 'Dry-Run (preview)',
    'run.cancel': 'Cancel',
    'run.cluster': 'Execute on cluster (SSH) — default: this server',
    'run.local': 'Local (this server)',
    'run.aiExplain': 'AI Explain',
    'run.explaining': 'Explaining…',
    'login.title': 'Sign in',
    'login.submit': 'Sign in',
    'login.username': 'Username',
    'login.password': 'Password',
    'login.signingIn': 'Signing in…',
    'lang.toggle': '中文',
  },
  zh: {
    'nav.dashboard': '仪表盘',
    'nav.editor': '流程编辑器',
    'nav.pipelines': '流程库',
    'nav.runs': '运行',
    'nav.chat': 'AI 对话',
    'nav.docs': 'API 文档',
    'nav.clusters': '集群',
    'nav.users': '用户',
    'nav.audit': '审计',
    'nav.settings': '设置',
    'nav.guest': '访客 — 登录',
    'nav.signedIn': '已登录',
    'dashboard.title': '你想做什么？',
    'dashboard.subtitle': '设计、运行并分享生信分析流程 — 每一步都有 AI 辅助。',
    'dashboard.ai': 'AI 生成流程',
    'dashboard.ai.desc': '用自然语言描述你的分析，让 AI 起草流程定义。',
    'dashboard.ai.disabledNote': 'AI 助手尚未配置',
    'dashboard.ai.disabledDesc': '在设置中启用 AI 提供商后，即可通过描述生成流程。',
    'dashboard.ai.disabledCta': '从模板开始',
    'dashboard.templates': '从模板开始',
    'dashboard.templates.desc': '久经实战的 RNA-seq、WGS、scRNA、QIIME2 等模板。',
    'dashboard.runs': '最近运行',
    'dashboard.noRuns': '还没有运行记录 — 完成的运行会出现在这里。',
    'dashboard.create': '打开编辑器',
    'dashboard.onboarding': '快速上手',
    'dashboard.onboarding.steps': [
      '用 AI 助手或模板创建流程',
      '先 Dry-Run 预览执行计划，再正式运行',
      '查看结果、下载文件、分享流程',
    ].join('|'),
    'dashboard.onboarding.dismiss': '知道了',
    'chat.disabled.message': '当前服务器未配置 AI 助手。你可以从模板构建流程，或逐步手动编写。',
    'chat.disabled.templates': '浏览模板',
    'chat.disabled.editor': '打开编辑器',
    'run.title': '运行选项',
    'run.parallelJobs': '并行任务数（最大）',
    'run.samples': '样本 — 仅运行指定子集（名称、first:N、ready；留空 = 全部）',
    'run.targets': '目标规则（逗号分隔；留空 = 引擎默认）',
    'run.keepGoing': '某个规则失败时继续运行',
    'run.dryRun': 'Dry-Run（预览）',
    'run.cancel': '取消',
    'run.cluster': '在集群（SSH）上执行 — 默认：本服务器',
    'run.local': '本服务器（本地）',
    'run.aiExplain': 'AI 解释',
    'run.explaining': '解释中…',
    'login.title': '登录',
    'login.submit': '登录',
    'login.username': '用户名',
    'login.password': '密码',
    'login.signingIn': '登录中…',
    'lang.toggle': 'EN',
  },
};

interface I18nValue {
  lang: Lang;
  setLang: (lang: Lang) => void;
  t: (key: string) => string;
}

const I18nContext = createContext<I18nValue | null>(null);

const LANG_STORAGE_KEY = 'oxo_lang';

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(() => {
    const saved = localStorage.getItem(LANG_STORAGE_KEY);
    return saved === 'zh' ? 'zh' : 'en';
  });

  const setLang = useCallback((next: Lang) => {
    localStorage.setItem(LANG_STORAGE_KEY, next);
    setLangState(next);
  }, []);

  const value = useMemo<I18nValue>(() => {
    const t = (key: string) => TRANSLATIONS[lang][key] ?? key;
    return { lang, setLang, t };
  }, [lang, setLang]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error('useI18n must be used inside I18nProvider');
  return ctx;
}
