import { test, expect } from '@playwright/test';
import { detectGuidedUnsupported } from '../src/utils/guidedSafety';

// Issue #1143 (audit UX-01) — table-driven unit coverage for the guided
// safety detector. Runs under Playwright (the frontend has no separate
// unit runner) but exercises the pure module directly, no browser.
//
// The 2026-11-18 review finding: header recognition must be total over
// TOML-legal spellings (`[[ rules ]]`, `[ engine ]`, `[rules."resources"]`)
// and header-shaped lines it cannot parse must fail closed. These cases
// pin that contract so a workflow span can never swallow a nested header.

interface Case {
  name: string;
  toml: string;
  /** Detector must return no findings. */
  empty?: boolean;
  contains?: string[];
  notContains?: string[];
}

const CASES: Case[] = [
  {
    name: 'basic workflow (modeled keys only) is allowed',
    toml: [
      '[workflow]',
      'name = "basic"',
      'version = "1.0.0"',
      'description = "kept"',
      '',
      '[[rules]]',
      'name = "a"',
      'input = ["{sample}.fq"]',
      'output = ["{sample}.bam"]',
      'shell = "x"',
      'threads = 2',
    ].join('\n'),
    empty: true,
  },
  {
    name: 'spaced [[ rules ]] header still exposes its rule fields',
    toml: [
      '[workflow]',
      'name = "spaced"',
      '',
      '[[ rules ]]',
      'name = "a"',
      'input = ["x.fq"]',
      'output = ["x.bam"]',
      'shell = "x"',
      'when = "config.on"',
      '',
      '[config]',
      'on = true',
    ].join('\n'),
    contains: ['rule "a" → when', '[config]'],
  },
  {
    name: 'spaced [ engine ] header is a section, not swallowed workflow body',
    toml: [
      '[workflow]',
      'name = "e"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      '',
      '[ engine ]',
      'min_free_disk = "1G"',
    ].join('\n'),
    contains: ['[engine]'],
  },
  {
    name: 'quoted [rules."resources"] sub-table is recognized',
    toml: [
      '[workflow]',
      'name = "q"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      '',
      '[rules."resources"]',
      'gpu = 1',
      'threads = 2',
    ].join('\n'),
    contains: ['rule "a" → [rules.resources] (gpu)'],
  },
  {
    name: 'quoted resources sub-table with only threads/memory is modeled',
    toml: [
      '[workflow]',
      'name = "q"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      '',
      '[rules."resources"]',
      'threads = 2',
      'memory = "8G"',
    ].join('\n'),
    empty: true,
  },
  {
    name: 'CRLF documents with spaced headers are read identically',
    toml: '[workflow]\r\nname = "c"\r\n\r\n[[ rules ]]\r\nname = "a"\r\nshell = "x"\r\nwhen = "config.on"\r\n\r\n[config]\r\non = true\r\n',
    contains: ['rule "a" → when', '[config]'],
  },
  {
    name: 'header-looking lines inside multi-line strings stay string content',
    toml: [
      '[workflow]',
      'name = "m"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = """',
      '[engine]',
      '[[ values ]]',
      '"""',
    ].join('\n'),
    empty: true,
  },
  {
    name: 'dotted [rules.resources] sub-table is recognized',
    toml: [
      '[workflow]',
      'name = "d"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      '',
      '[rules.resources]',
      'time_limit = "1h"',
    ].join('\n'),
    contains: ['rule "a" → [rules.resources] (time_limit)'],
  },
  {
    name: 'mixed indexed and spaced headers attribute fields to the right rule',
    toml: [
      '[workflow]',
      'name = "mixed"',
      '',
      '[[rules]]',
      'name = "indexed"',
      'shell = "x"',
      '',
      '[[ rules ]]',
      'name = "unindexed"',
      'input = ["a"]',
      'output = ["b"]',
      'shell = "y"',
      'depends_on = ["indexed"]',
    ].join('\n'),
    contains: ['rule "unindexed" → depends_on'],
    notContains: ['rule "indexed" →'],
  },
  {
    name: 'unparseable header-shaped line fails closed',
    toml: [
      '[workflow]',
      'name = "bad"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      '',
      '[rules..resources]',
      'gpu = 1',
    ].join('\n'),
    contains: ['unrecognized table header at line 8: [rules..resources]'],
  },
  {
    name: 'whitespace around a dotted statement key is normalized',
    toml: [
      '[workflow]',
      'name = "k"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      'resources . gpu = 1',
    ].join('\n'),
    contains: ['rule "a" → resources.gpu'],
  },
  {
    name: 'escaped quoted statement key normalizes to its semantic name',
    toml: [
      '[workflow]',
      'name = "k"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
      '"whe\\u006e" = "config.on"',
    ].join('\n'),
    contains: ['rule "a" → when'],
  },
  {
    name: 'empty document has nothing to lose',
    toml: '',
    empty: true,
  },
  {
    name: 'root-level keys are unmodeled',
    toml: [
      'reference_dir = "ref"',
      '',
      '[workflow]',
      'name = "r"',
      '',
      '[[rules]]',
      'name = "a"',
      'shell = "x"',
    ].join('\n'),
    contains: ['top-level key "reference_dir"'],
  },
];

test.describe('Guided safety detector (UX-01)', () => {
  for (const c of CASES) {
    test(c.name, () => {
      const labels = detectGuidedUnsupported(c.toml);
      if (c.empty) {
        expect(labels, `expected no findings, got ${JSON.stringify(labels)}`).toEqual([]);
      }
      for (const expected of c.contains ?? []) {
        expect(labels, `findings: ${JSON.stringify(labels)}`).toContain(expected);
      }
      for (const forbidden of c.notContains ?? []) {
        expect(labels, `findings: ${JSON.stringify(labels)}`).not.toContain(forbidden);
      }
    });
  }
});
