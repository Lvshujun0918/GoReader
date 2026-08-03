<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { deleteReplaceRule, getReplaceRules, saveReplaceRule } from '@/api/replaceRules'
import { useUserStore } from '@/stores/user'
import type { ReplaceRule } from '@/types'

const router = useRouter()
const store = useUserStore()

/* ================= 列表（localStorage: reader_replace_rules，见 api/replaceRules.ts 契约注释） ================= */
const rules = ref<ReplaceRule[]>([])
const loading = ref(true)

async function load() {
  loading.value = true
  try {
    const res = await getReplaceRules()
    rules.value = res.data ?? []
  } catch {
    rules.value = []
  } finally {
    loading.value = false
  }
}

const enabledCount = computed(() => rules.value.filter((r) => r.enabled).length)

function newId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
}

/* ================= 新增 / 编辑弹窗 ================= */
const editorOpen = ref(false)
const editorBusy = ref(false)
const editingId = ref<string | null>(null)
const form = ref({ name: '', find: '', replace: '', enabled: true })

function openAdd() {
  editingId.value = null
  form.value = { name: '', find: '', replace: '', enabled: true }
  editorOpen.value = true
  document.body.style.overflow = 'hidden'
}

function openEdit(r: ReplaceRule) {
  editingId.value = r.id
  form.value = {
    name: r.name ?? '',
    find: r.find ?? '',
    replace: r.replace ?? '',
    enabled: r.enabled,
  }
  editorOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeEditor() {
  if (editorBusy.value) return
  editorOpen.value = false
  document.body.style.overflow = ''
}

async function confirmSave() {
  if (editorBusy.value) return
  const find = form.value.find.trim()
  if (!find) {
    ElMessage.warning('「查找」内容不能为空')
    return
  }
  editorBusy.value = true
  const editing = editingId.value
  const rule: ReplaceRule = {
    id: editing ?? newId(),
    name: form.value.name.trim() || find,
    find,
    replace: form.value.replace,
    enabled: form.value.enabled,
    order: editing ? (rules.value.find((r) => r.id === editing)?.order ?? 0) : rules.value.length,
  }
  try {
    // 当前为 localStorage 占位；后端就绪后走 POST /reader3/saveReplaceRule（见 api/replaceRules.ts）
    await saveReplaceRule(rule)
    if (editing) {
      const i = rules.value.findIndex((r) => r.id === editing)
      if (i >= 0) rules.value[i] = rule
    } else {
      rules.value.push(rule)
    }
    closeEditor()
  } finally {
    editorBusy.value = false
  }
}

/* ================= 启用开关 ================= */
const toggling = ref<Set<string>>(new Set())

async function toggleRule(r: ReplaceRule) {
  if (toggling.value.has(r.id)) return
  toggling.value.add(r.id)
  const prev = r.enabled
  r.enabled = !prev // 乐观更新
  try {
    await saveReplaceRule({ ...r, enabled: !prev })
  } catch {
    r.enabled = prev // 失败回滚
  } finally {
    toggling.value.delete(r.id)
  }
}

/* ================= 删除（极简确认弹窗） ================= */
const deleting = ref<ReplaceRule | null>(null)
const deleteBusy = ref(false)

function askDelete(r: ReplaceRule) {
  deleting.value = r
  document.body.style.overflow = 'hidden'
}

async function confirmDelete() {
  const r = deleting.value
  if (!r || deleteBusy.value) return
  deleteBusy.value = true
  try {
    // 当前为 localStorage 占位；后端就绪后走 POST /reader3/deleteReplaceRule（见 api/replaceRules.ts）
    await deleteReplaceRule(r.id)
    rules.value = rules.value.filter((x) => x.id !== r.id)
    closeDelete()
  } catch {
    // 已提示
  } finally {
    deleteBusy.value = false
  }
}

function closeDelete() {
  deleting.value = null
  document.body.style.overflow = ''
}

onMounted(load)
</script>

<template>
  <div class="rules-page">
    <!-- 极简导航：字标 + 页面入口 -->
    <header class="topbar">
      <div class="brand">
        <LogoMark class="brand-logo" />
        <span class="brand-name">夜读<span class="brand-dot">.</span></span>
      </div>

      <div class="user-area">
        <button class="nav-link" type="button" @click="router.push('/')">书架</button>
        <button class="nav-link" type="button" @click="router.push('/search')">搜索</button>
        <button class="nav-link" type="button" @click="router.push('/sources')">书源</button>
        <button class="nav-link active" type="button" @click="router.push('/rules')">替换规则</button>
        <button class="nav-link" type="button" @click="router.push('/settings')">设置</button>
        <span class="user-chip">{{ store.username || '未登录' }}</span>
      </div>
    </header>

    <main class="content">
      <div class="section-head">
        <h1 class="section-title">替换规则</h1>
        <span class="count">{{ rules.length }} 条 · {{ enabledCount }} 启用</span>
        <button class="add-btn" type="button" @click="openAdd">新增规则</button>
      </div>

      <!-- 加载态 -->
      <div v-if="loading" class="state-row">
        <p class="state-text">加载中…</p>
      </div>

      <!-- 空状态 -->
      <div v-else-if="rules.length === 0" class="state-row">
        <p class="state-text">暂无规则，点击右上角新增</p>
      </div>

      <!-- 规则列表（极简表格） -->
      <div v-else class="table-wrap">
        <table class="rule-table">
          <thead>
            <tr>
              <th class="th-name">名称</th>
              <th class="th-find">查找</th>
              <th class="th-replace">替换</th>
              <th class="th-enabled">启用</th>
              <th class="th-ops">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="r in rules" :key="r.id">
              <td class="td-name" :title="r.name">{{ r.name }}</td>
              <td class="td-find mono" :title="r.find">{{ r.find }}</td>
              <td class="td-replace mono" :title="r.replace">{{ r.replace || '—' }}</td>
              <td class="td-enabled">
                <button
                  class="switch"
                  :class="{ on: r.enabled }"
                  type="button"
                  role="switch"
                  :aria-checked="r.enabled"
                  :title="r.enabled ? '停用' : '启用'"
                  @click="toggleRule(r)"
                >
                  <span class="switch-knob"></span>
                </button>
              </td>
              <td class="td-ops">
                <button class="op-btn" type="button" @click="openEdit(r)">编辑</button>
                <button class="op-btn danger" type="button" @click="askDelete(r)">删除</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <p class="foot-tip">规则保存在本地浏览器（后端就绪后自动切换为账号同步，见 api/replaceRules.ts 契约注释）</p>
    </main>

    <!-- 新增 / 编辑规则弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="editorOpen" class="dlg-overlay" @click.self="closeEditor">
          <div class="dlg" role="dialog" aria-modal="true" aria-label="编辑替换规则" tabindex="-1" @keydown.esc="closeEditor">
            <div class="dlg-head">
              <h2 class="dlg-title">{{ editingId ? '编辑规则' : '新增规则' }}</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="editorBusy" @click="closeEditor">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <form class="dlg-form" @submit.prevent="confirmSave">
              <label class="field">
                <span class="field-label">名称</span>
                <input v-model="form.name" class="field-input" type="text" placeholder="留空则使用「查找」内容" maxlength="40" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">查找<em>*</em></span>
                <input v-model="form.find" class="field-input" type="text" placeholder="要被替换的文字（必填）" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">替换</span>
                <input v-model="form.replace" class="field-input" type="text" placeholder="替换为（可留空 = 删除匹配文字）" spellcheck="false" />
              </label>
              <div class="field">
                <span class="field-label">启用</span>
                <button
                  class="switch"
                  :class="{ on: form.enabled }"
                  type="button"
                  role="switch"
                  :aria-checked="form.enabled"
                  @click="form.enabled = !form.enabled"
                >
                  <span class="switch-knob"></span>
                </button>
              </div>
              <p class="field-tip">正文渲染时按顺序逐条 replaceAll（全文匹配，非正则）</p>
              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="editorBusy" @click="closeEditor">取消</button>
                <button class="accent-btn" type="submit" :disabled="editorBusy || !form.find.trim()">
                  {{ editorBusy ? '保存中…' : '保存' }}
                </button>
              </div>
            </form>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 删除确认弹窗（极简） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="deleting" class="dlg-overlay" @click.self="closeDelete">
          <div class="dlg dlg-confirm" role="alertdialog" aria-modal="true" aria-label="删除规则" tabindex="-1" @keydown.esc="closeDelete">
            <div class="dlg-head">
              <h2 class="dlg-title">删除规则</h2>
            </div>
            <p class="confirm-text">确定删除「{{ deleting.name }}」吗？此操作不可恢复。</p>
            <div class="dlg-actions">
              <button class="ghost-btn" type="button" :disabled="deleteBusy" @click="closeDelete">取消</button>
              <button class="danger-btn" type="button" :disabled="deleteBusy" @click="confirmDelete">
                {{ deleteBusy ? '删除中…' : '删除' }}
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.rules-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  animation: fade-in 0.2s ease both;
}

/* ================= 顶部导航 ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 24px;
  padding: 14px 32px;
  background: rgba(250, 250, 250, 0.86);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.brand-logo {
  width: 26px;
  height: 26px;
}
.brand-name {
  font-size: 17px;
  font-weight: 300;
  letter-spacing: 3px;
  color: var(--text-1);
}
.brand-dot {
  color: var(--accent);
  font-weight: 400;
}
.user-area {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-left: auto;
  flex-shrink: 0;
}
.nav-link {
  padding: 5px 2px;
  border: none;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.nav-link:hover,
.nav-link.active {
  color: var(--accent);
}
.user-chip {
  font-size: 13px;
  font-weight: 400;
  color: var(--text-2);
}

/* ================= 内容区 ================= */
.content {
  width: min(860px, 100%);
  margin: 0 auto;
  padding: 44px 32px 72px;
}
.section-head {
  display: flex;
  align-items: baseline;
  gap: 14px;
  margin-bottom: 26px;
}
.section-title {
  margin: 0;
  font-size: 22px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-1);
}
.count {
  font-size: 12.5px;
  font-weight: 300;
  color: var(--text-3);
}
.add-btn {
  margin-left: auto;
  padding: 7px 18px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: none;
  color: var(--accent);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.add-btn:hover {
  color: var(--accent-deep);
  border-color: var(--accent-deep);
  background: var(--accent-soft);
}

/* ================= 状态行 ================= */
.state-row {
  padding: 72px 0;
  text-align: center;
}
.state-text {
  margin: 0;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 2px;
  color: var(--text-3);
}

/* ================= 极简表格 ================= */
.table-wrap {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
  background: var(--surface);
}
.rule-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}
.rule-table th {
  padding: 12px 16px;
  text-align: left;
  font-size: 12px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--text-3);
  border-bottom: 1px solid var(--border);
  background: var(--bg);
}
.rule-table td {
  padding: 13px 16px;
  border-bottom: 1px solid var(--border);
  color: var(--text-2);
  vertical-align: middle;
}
.rule-table tr:last-child td {
  border-bottom: none;
}
.rule-table tbody tr {
  transition: background-color 0.15s ease;
}
.rule-table tbody tr:hover {
  background: var(--hover);
}
.th-name {
  width: 26%;
}
.th-enabled,
.th-ops {
  width: 90px;
}
.td-name {
  color: var(--text-1);
  font-weight: 400;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 0;
}
.td-find,
.td-replace {
  max-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mono {
  font-family: 'SF Mono', 'JetBrains Mono', Consolas, monospace;
  font-size: 12px;
}
.td-enabled,
.td-ops {
  text-align: center;
}

/* 极简开关 */
.switch {
  position: relative;
  flex-shrink: 0;
  width: 36px;
  height: 20px;
  border-radius: 999px;
  border: 1px solid var(--border-strong);
  background: none;
  cursor: pointer;
  transition:
    border-color 0.2s ease,
    background-color 0.2s ease;
  vertical-align: middle;
}
.switch .switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--text-3);
  transition:
    transform 0.2s ease,
    background-color 0.2s ease;
}
.switch:hover {
  border-color: var(--accent);
}
.switch.on {
  border-color: var(--accent);
  background: var(--accent-soft);
}
.switch.on .switch-knob {
  transform: translateX(16px);
  background: var(--accent);
}

/* 操作按钮 */
.op-btn {
  padding: 3px 10px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12px;
  font-weight: 400;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.op-btn + .op-btn {
  margin-left: 6px;
}
.op-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.op-btn.danger:hover {
  color: #cf4444;
  border-color: #cf4444;
}

.foot-tip {
  margin: 18px 2px 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}

/* ================= 弹窗 ================= */
.dlg-overlay {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(24, 24, 27, 0.35);
}
.dlg {
  width: min(420px, 100%);
  padding: 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.08);
  outline: none;
}
.dlg-confirm {
  width: min(360px, 100%);
}
.dlg-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;
}
.dlg-title {
  margin: 0;
  font-size: 15px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--text-1);
}
.dlg-close {
  width: 26px;
  height: 26px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease;
}
.dlg-close:hover:not(:disabled) {
  color: var(--text-1);
  background: var(--hover);
}
.dlg-close:disabled {
  cursor: not-allowed;
  opacity: 0.4;
}
.dlg-close svg {
  width: 13px;
  height: 13px;
}
.dlg-form {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.field-label {
  font-size: 12.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-2);
}
.field-label em {
  font-style: normal;
  color: #cf4444;
  margin-left: 2px;
}
.field-input {
  height: 36px;
  padding: 0 12px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--bg);
  color: var(--text-1);
  font-family: inherit;
  font-size: 13px;
  font-weight: 400;
  outline: none;
  transition: border-color 0.2s ease;
}
.field-input::placeholder {
  color: var(--text-3);
  font-weight: 300;
}
.field-input:focus {
  border-color: var(--accent);
  background: var(--surface);
}
.field-tip {
  margin: -4px 0 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.dlg-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 6px;
}
.ghost-btn {
  padding: 7px 16px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.ghost-btn:hover:not(:disabled) {
  color: var(--text-1);
  border-color: var(--border-strong);
}
.accent-btn {
  padding: 7px 18px;
  border-radius: var(--radius);
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #ffffff;
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    background-color 0.2s ease,
    border-color 0.2s ease;
}
.accent-btn:hover:not(:disabled) {
  background: var(--accent-deep);
  border-color: var(--accent-deep);
}
.ghost-btn:disabled,
.accent-btn:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}
.danger-btn {
  padding: 7px 18px;
  border-radius: var(--radius);
  border: 1px solid #cf4444;
  background: none;
  color: #cf4444;
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    background-color 0.2s ease,
    color 0.2s ease;
}
.danger-btn:hover:not(:disabled) {
  background: rgba(207, 68, 68, 0.08);
}
.danger-btn:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}
.confirm-text {
  margin: 0 0 18px;
  font-size: 13px;
  font-weight: 300;
  line-height: 1.7;
  color: var(--text-2);
}

/* 弹窗动画：fade 200ms */
.dlg-enter-active,
.dlg-leave-active {
  transition: opacity 0.2s ease;
}
.dlg-enter-from,
.dlg-leave-to {
  opacity: 0;
}
.dlg-enter-active .dlg,
.dlg-leave-active .dlg {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}
.dlg-enter-from .dlg,
.dlg-leave-to .dlg {
  opacity: 0;
  transform: translateY(6px);
}

/* ================= 响应式 ================= */
@media (max-width: 720px) {
  .topbar {
    flex-wrap: wrap;
    gap: 12px;
    padding: 12px 16px;
  }
  .content {
    padding: 32px 16px 56px;
  }
  .rule-table th,
  .rule-table td {
    padding: 10px 12px;
  }
  .th-replace {
    display: none;
  }
  .td-replace {
    display: none;
  }
  .dlg-overlay {
    padding: 16px;
  }
}
</style>
