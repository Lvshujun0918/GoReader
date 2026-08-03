<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { deleteHttpTts, getHttpTtsList, saveHttpTts } from '@/api/httpTts'
import { backupToWebdav } from '@/api/backup'
import { useUserStore } from '@/stores/user'
import type { HttpTts } from '@/types'

const router = useRouter()
const store = useUserStore()

/** 版本号与 package.json 保持一致 */
const VERSION = '0.1.0'

const showToken = ref(false)

function maskToken(t: string): string {
  if (!t) return '未登录'
  if (t.length <= 12) return `${t.slice(0, 4)}…`
  return `${t.slice(0, 8)}…${t.slice(-4)}`
}

async function logout() {
  try {
    await ElMessageBox.confirm('确定退出登录吗？', '退出登录', {
      confirmButtonText: '退出',
      cancelButtonText: '取消',
      type: 'warning',
    })
  } catch {
    return // 用户取消
  }
  store.clear() // 清空 localStorage（reader_access_token / reader_username）
  ElMessage.success('已退出登录')
  void router.replace('/login')
}
/* ================= 听书设置（HttpTTS，localStorage 占位，见 api/httpTts.ts 契约注释） ================= */

const TTS_TYPE_LABEL: Record<number, string> = { 0: '在线合成', 1: '本地引擎' }
const ttsList = ref<HttpTts[]>([])

async function loadTtsList() {
  try {
    const res = await getHttpTtsList()
    ttsList.value = res.data ?? []
  } catch {
    ttsList.value = []
  }
}

function ttsTypeLabel(t: number): string {
  return TTS_TYPE_LABEL[t] ?? `类型 ${t}`
}

function newTtsId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
}

/* 新增弹窗 */
const ttsDialogOpen = ref(false)
const ttsBusy = ref(false)
const ttsForm = ref<{ name: string; url: string; type: number }>({ name: '', url: '', type: 0 })

function openAddTts() {
  ttsForm.value = { name: '', url: '', type: 0 }
  ttsDialogOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeAddTts() {
  if (ttsBusy.value) return
  ttsDialogOpen.value = false
  document.body.style.overflow = ''
}

async function confirmAddTts() {
  if (ttsBusy.value) return
  const url = ttsForm.value.url.trim()
  if (!url) {
    ElMessage.warning('URL 不能为空')
    return
  }
  ttsBusy.value = true
  try {
    // 当前为 localStorage 占位；后端就绪后走 POST /reader3/saveHttpTTS（见 api/httpTts.ts）
    await saveHttpTts({
      id: newTtsId(),
      name: ttsForm.value.name.trim() || url,
      url,
      type: ttsForm.value.type,
    })
    await loadTtsList()
    closeAddTts()
  } finally {
    ttsBusy.value = false
  }
}

/* 删除 */
const deletingTts = ref<HttpTts | null>(null)
const deleteTtsBusy = ref(false)

function askDeleteTts(t: HttpTts) {
  deletingTts.value = t
  document.body.style.overflow = 'hidden'
}

async function confirmDeleteTts() {
  const t = deletingTts.value
  if (!t || deleteTtsBusy.value) return
  deleteTtsBusy.value = true
  try {
    // 当前为 localStorage 占位；后端就绪后走 POST /reader3/deleteHttpTTS（见 api/httpTts.ts）
    await deleteHttpTts(t.id)
    ttsList.value = ttsList.value.filter((x) => x.id !== t.id)
    closeDeleteTts()
  } catch {
    // 已提示
  } finally {
    deleteTtsBusy.value = false
  }
}

function closeDeleteTts() {
  deletingTts.value = null
  document.body.style.overflow = ''
}

onMounted(loadTtsList)

/* ================= OPDS 访问 ================= */
/** OPDS 地址 = 当前 host + /opds */
const opdsUrl = `${window.location.origin}/opds`
const opdsCopied = ref(false)

async function copyOpdsUrl() {
  const text = opdsUrl
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    // 剪贴板 API 不可用（非 https 等）：textarea 降级
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    document.execCommand('copy')
    document.body.removeChild(ta)
  }
  opdsCopied.value = true
  window.setTimeout(() => (opdsCopied.value = false), 1600)
}

/* ================= 数据备份（WebDAV） ================= */
const backupBusy = ref(false)
const backupPath = ref('')

async function runBackup() {
  if (backupBusy.value) return
  backupBusy.value = true
  backupPath.value = ''
  try {
    const res = await backupToWebdav()
    backupPath.value = res.data?.path ?? ''
    if (!backupPath.value) {
      ElMessage.warning('备份完成，但未返回文件路径')
    } else {
      ElMessage.success('备份完成')
    }
  } catch {
    // 错误提示已由拦截器处理
  } finally {
    backupBusy.value = false
  }
}
</script>

<template>
  <div class="settings-page">
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
        <button class="nav-link" type="button" @click="router.push('/rules')">替换规则</button>
        <button class="nav-link active" type="button" @click="router.push('/settings')">设置</button>
        <span class="user-chip">{{ store.username || '未登录' }}</span>
      </div>
    </header>

    <main class="content">
      <div class="section-head">
        <h1 class="section-title">设置</h1>
        <span class="count">v{{ VERSION }}</span>
      </div>

      <!-- 账号信息 -->
      <section class="card">
        <h2 class="card-title">账号信息</h2>
        <div class="row">
          <span class="row-label">用户名</span>
          <span class="row-value">{{ store.username || '未登录' }}</span>
        </div>
        <div class="row">
          <span class="row-label">Token</span>
          <span class="row-value mono">{{ showToken ? store.accessToken : maskToken(store.accessToken) }}</span>
          <button
            v-if="store.accessToken"
            class="row-action"
            type="button"
            @click="showToken = !showToken"
          >
            {{ showToken ? '隐藏' : '显示' }}
          </button>
        </div>
        <div class="card-foot">
          <button class="danger-btn" type="button" @click="logout">退出登录</button>
        </div>
      </section>

      <!-- 主题偏好 -->
      <section class="card">
        <h2 class="card-title">主题偏好</h2>
        <div class="row">
          <span class="row-label">主题</span>
          <span class="row-value">浅色 / 深色 / 纸色</span>
          <span class="row-hint">请在阅读页顶部切换</span>
        </div>
      </section>

      <!-- 听书设置 -->
      <section class="card">
        <div class="card-head">
          <h2 class="card-title">听书设置</h2>
          <span class="card-sub">HttpTTS 朗读源 · 本地占位存储（后端就绪后同步，见 api/httpTts.ts 契约注释）</span>
          <button class="row-action" type="button" @click="openAddTts">新增听书源</button>
        </div>
        <ul v-if="ttsList.length" class="tts-list">
          <li v-for="t in ttsList" :key="t.id" class="tts-row">
            <span class="tts-name" :title="t.name">{{ t.name }}</span>
            <span class="tts-url mono" :title="t.url">{{ t.url }}</span>
            <span class="tts-type">{{ ttsTypeLabel(t.type) }}</span>
            <button class="tts-del" type="button" title="删除听书源" @click="askDeleteTts(t)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M4 7h16" />
                <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                <path d="M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4l.8-12" />
              </svg>
            </button>
          </li>
        </ul>
        <p v-else class="tts-empty">暂无听书源。当前阅读页「听书」使用浏览器自带语音朗读。</p>
        <div class="row">
          <span class="row-label">阅读页听书</span>
          <span class="row-value">浏览器语音（SpeechSynthesis）</span>
          <span class="row-hint">阅读页顶栏「听书」按钮</span>
        </div>
      </section>

      <!-- OPDS 访问 -->
      <section class="card">
        <h2 class="card-title">OPDS 访问</h2>
        <div class="row">
          <span class="row-label">OPDS 地址</span>
          <span class="row-value mono">{{ opdsUrl }}</span>
          <button class="row-action" type="button" @click="copyOpdsUrl">
            {{ opdsCopied ? '已复制' : '复制' }}
          </button>
        </div>
        <p class="card-note">外部阅读器（如 legado、静读天下等）可通过此地址连接书架，账号密码与本应用登录一致。</p>
      </section>

      <!-- 数据备份 -->
      <section class="card">
        <h2 class="card-title">数据备份</h2>
        <div class="row">
          <span class="row-label">WebDAV 备份</span>
          <span class="row-value">{{ backupBusy ? '备份中…' : '备份到 WebDAV legado 目录' }}</span>
          <button class="row-action" type="button" :disabled="backupBusy" @click="runBackup">
            {{ backupBusy ? '备份中…' : '立即备份' }}
          </button>
        </div>
        <p v-if="backupPath" class="card-note mono backup-path">已备份至：{{ backupPath }}</p>
      </section>

      <!-- txtTocRule -->
      <section class="card">
        <h2 class="card-title">txtTocRule</h2>
        <div class="row">
          <span class="row-label">txtTocRule</span>
          <span class="row-value">后端实现中 · 替换规则页在 /rules</span>
          <button class="row-action" type="button" @click="router.push('/rules')">前往</button>
        </div>
        <p class="card-note">TXT 文本目录解析规则（txtTocRule）后端实现中；现有「替换规则」在 /rules 页维护，txtTocRule 就绪后将在此一并管理。</p>
      </section>

      <!-- 关于 -->
      <section class="card">
        <h2 class="card-title">关于</h2>
        <div class="row">
          <span class="row-label">应用</span>
          <span class="row-value">夜读 READER</span>
        </div>
        <div class="row">
          <span class="row-label">版本</span>
          <span class="row-value">v{{ VERSION }}</span>
        </div>
      </section>
    </main>
    <!-- 新增听书源弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="ttsDialogOpen" class="dlg-overlay" @click.self="closeAddTts">
          <div class="dlg" role="dialog" aria-modal="true" aria-label="新增听书源" tabindex="-1" @keydown.esc="closeAddTts">
            <div class="dlg-head">
              <h2 class="dlg-title">新增听书源</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="ttsBusy" @click="closeAddTts">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <form class="dlg-form" @submit.prevent="confirmAddTts">
              <label class="field">
                <span class="field-label">URL<em>*</em></span>
                <input v-model="ttsForm.url" class="field-input" type="text" placeholder="https://…/tts?text=" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">名称</span>
                <input v-model="ttsForm.name" class="field-input" type="text" placeholder="留空则使用 URL" maxlength="40" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">类型</span>
                <select v-model.number="ttsForm.type" class="field-input field-select">
                  <option :value="0">0 · 在线合成</option>
                  <option :value="1">1 · 本地引擎</option>
                </select>
              </label>
              <p class="field-tip">后端就绪后接入朗读（契约 POST /reader3/saveHttpTTS，见 api/httpTts.ts）</p>
              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="ttsBusy" @click="closeAddTts">取消</button>
                <button class="accent-btn" type="submit" :disabled="ttsBusy || !ttsForm.url.trim()">
                  {{ ttsBusy ? '保存中…' : '保存' }}
                </button>
              </div>
            </form>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 删除听书源确认弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="deletingTts" class="dlg-overlay" @click.self="closeDeleteTts">
          <div class="dlg dlg-confirm" role="alertdialog" aria-modal="true" aria-label="删除听书源" tabindex="-1" @keydown.esc="closeDeleteTts">
            <div class="dlg-head">
              <h2 class="dlg-title">删除听书源</h2>
            </div>
            <p class="confirm-text">确定删除「{{ deletingTts.name }}」吗？此操作不可恢复。</p>
            <div class="dlg-actions">
              <button class="ghost-btn" type="button" :disabled="deleteTtsBusy" @click="closeDeleteTts">取消</button>
              <button class="danger-btn" type="button" :disabled="deleteTtsBusy" @click="confirmDeleteTts">
                {{ deleteTtsBusy ? '删除中…' : '删除' }}
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.settings-page {
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

/* 用户区 */
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
  width: min(720px, 100%);
  margin: 0 auto;
  padding: 48px 32px 72px;
}
.section-head {
  display: flex;
  align-items: baseline;
  gap: 14px;
  margin-bottom: 32px;
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

/* ================= 卡片分区 ================= */
.card {
  margin-bottom: 28px;
  padding: 22px 24px 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
}
.card-title {
  margin: 0 0 6px;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 1px;
  color: var(--text-3);
}
.card-head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 4px;
}
.card-head .card-title {
  margin: 0;
}
.card-sub {
  flex: 1;
  min-width: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}

/* 听书源列表 */
.tts-list {
  list-style: none;
  margin: 8px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.tts-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 9px 12px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--bg);
}
.tts-name {
  flex-shrink: 0;
  max-width: 140px;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tts-url {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tts-type {
  flex-shrink: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.tts-del {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 4px;
  background: none;
  color: var(--text-3);
  cursor: pointer;
  transition:
    color 0.2s ease,
    background-color 0.2s ease;
}
.tts-del:hover {
  color: #cf4444;
  background: rgba(207, 68, 68, 0.08);
}
.tts-del svg {
  width: 12px;
  height: 12px;
}
.tts-empty {
  margin: 12px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
}

/* ================= 弹窗（新增 / 删除听书源） ================= */
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
.field-select {
  cursor: pointer;
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
.row {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 13px 0;
  border-bottom: 1px solid var(--border);
}
.row:last-of-type {
  border-bottom: none;
}
.row-label {
  flex-shrink: 0;
  width: 72px;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-2);
}
.row-value {
  flex: 1;
  min-width: 0;
  font-size: 13.5px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.row-value.mono {
  font-family: 'SF Mono', 'JetBrains Mono', Consolas, monospace;
  font-size: 12px;
}
.mono {
  font-family: 'SF Mono', 'JetBrains Mono', Consolas, monospace;
}
.row-hint {
  flex-shrink: 0;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.row-action {
  flex-shrink: 0;
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
.row-action:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.card-foot {
  display: flex;
  justify-content: flex-end;
  padding-top: 16px;
}
.card-note {
  margin: 10px 0 0;
  font-size: 11.5px;
  font-weight: 300;
  line-height: 1.7;
  color: var(--text-3);
}
.card-note.mono {
  font-family: 'SF Mono', 'JetBrains Mono', Consolas, monospace;
}
.backup-path {
  color: var(--accent);
}
.row-action:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}
.danger-btn {
  padding: 8px 20px;
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
    color 0.2s ease,
    background-color 0.2s ease,
    border-color 0.2s ease;
}
.danger-btn:hover {
  color: #ffffff;
  background: #cf4444;
  border-color: #cf4444;
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
  .card {
    padding: 18px 16px;
  }
  .row-hint {
    display: none;
  }
}
</style>
