import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  filterCommands,
  paletteCommands,
} from './commandPalette.ts'

test('命令表：跳转页面 + 设置项齐全（书架/规则/设置/用户/深色）', () => {
  const cmds = paletteCommands()
  const ids = cmds.map((c) => c.id)
  // 跳转页面
  for (const path of ['/', '/rules', '/settings', '/users']) {
    assert.ok(ids.includes(`nav-${path}`), `缺少导航命令 ${path}`)
  }
  // 设置项：深色
  assert.ok(ids.includes('theme-dark'))
  assert.ok(ids.includes('theme-light'))
  assert.ok(ids.includes('theme-system'))
  // 分组顺序：页面组在设置组前
  const groups = cmds.map((c) => c.group)
  assert.ok(groups.indexOf('跳转页面') < groups.indexOf('打开设置'))
})

test('空输入返回全部命令', () => {
  assert.equal(filterCommands('').length, paletteCommands().length)
  assert.equal(filterCommands('   ').length, paletteCommands().length)
})

test('按标题/关键词过滤（忽略大小写，空格分词 AND）', () => {
  const dark = filterCommands('深色')
  assert.ok(dark.length > 0)
  assert.ok(dark.some((c) => c.id === 'theme-dark'))

  const both = filterCommands('规则')
  assert.ok(both.length > 0)
  assert.ok(both.every((c) => `${c.title} ${c.keywords.join(' ')}`.toLowerCase().includes('规则')))

  assert.equal(filterCommands('不存在的词xyz').length, 0)
})

test('导航命令 path 正确', () => {
  const cmds = paletteCommands()
  const nav = cmds.find((c) => c.id === 'nav-/rules')
  assert.deepEqual(nav?.action, { kind: 'navigate', path: '/rules' })
})
