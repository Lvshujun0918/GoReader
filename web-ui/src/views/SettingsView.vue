<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { deleteHttpTts, getHttpTtsList, saveHttpTts } from '@/api/httpTts'
import { backupToWebdav } from '@/api/backup'
import { getSystemInfo } from '@/api/system'
import { deleteTxtTocRule, getTxtTocRules, importDefaultTxtTocRules, saveTxtTocRule } from '@/api/txtTocRules'
import { useUserStore } from '@/stores/user'
import type { HttpTts, SystemInfo, TxtTocRule } from '@/types'

const router = useRouter()
const store = useUserStore()

/** 版本号与 package.json 保持一致 */
const VERSION = '0.1.0'

/** 系统信息（/reader3/getSystemInfo，设置页「关于」区展示） */
const sysInfo = ref<SystemInfo | null>(null)

async function loadSysInfo() {
  try {
    const res = await getSystemInfo()
    sysInfo.value = res.data ?? null
  } catch {
    sysInfo.value = null // 后端不可用时静默（版本仍显示前端常量）
  }
}

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

/* ================= txtTocRule（自定义 TXT 目录规则，后端 /reader3/getTxtTocRules 等） ================= */
const tocRules = ref<TxtTocRule[]>([])
const tocLoading = ref(true)

async function loadTxtTocRules() {
  tocLoading.value = true
  try {
    const res = await getTxtTocRules()
    tocRules.value = res.data ?? []
  } catch {
    tocRules.value = []
  } finally {
    tocLoading.value = false
  }
}

const customTocRules = computed(() => tocRules.value.filter((r) => !r.id.startsWith('default-')))

function newTocId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
}

/* 新增弹窗 */
const tocDialogOpen = ref(false)
const tocBusy = ref(false)
const tocForm = ref<{ name: string; rule: string; enable: boolean }>({ name: '', rule: '', enable: true })

function openAddToc() {
  tocForm.value = { name: '', rule: '', enable: true }
  tocDialogOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeAddToc() {
  if (tocBusy.value) return
  tocDialogOpen.value = false
  document.body.style.overflow = ''
}

async function confirmAddToc() {
  if (tocBusy.value) return
  const rule = tocForm.value.rule.trim()
  if (!rule) {
    ElMessage.warning('规则正则不能为空')
    return
  }
  tocBusy.value = true
  try {
    await saveTxtTocRule({
      id: newTocId(),
      name: tocForm.value.name.trim() || rule,
      rule,
      enable: tocForm.value.enable,
      serialNumber: customTocRules.value.length,
    })
    await loadTxtTocRules()
    closeAddToc()
  } finally {
    tocBusy.value = false
  }
}

/* 启用开关（默认规则只读，仅自定义规则可切换） */
const tocToggling = ref<Set<string>>(new Set())

async function toggleTocRule(r: TxtTocRule) {
  if (tocToggling.value.has(r.id) || r.id.startsWith('default-')) return
  tocToggling.value.add(r.id)
  const prev = r.enable
  r.enable = !prev
  try {
    await saveTxtTocRule({ ...r, enable: !prev })
  } catch {
    r.enable = prev
  } finally {
    tocToggling.value.delete(r.id)
  }
}

/* 删除（仅自定义规则） */
const deletingToc = ref<TxtTocRule | null>(null)
const deleteTocBusy = ref(false)

function askDeleteToc(r: TxtTocRule) {
  if (r.id.startsWith('default-')) return
  deletingToc.value = r
  document.body.style.overflow = 'hidden'
}

async function confirmDeleteToc() {
  const r = deletingToc.value
  if (!r || deleteTocBusy.value) return
  deleteTocBusy.value = true
  try {
    await deleteTxtTocRule(r.id)
    tocRules.value = tocRules.value.filter((x) => x.id !== r.id)
    closeDeleteToc()
  } catch {
    // 已提示
  } finally {
    deleteTocBusy.value = false
  }
}

function closeDeleteToc() {
  deletingToc.value = null
  document.body.style.overflow = ''
}

/* 导入默认规则 */
const tocImportBusy = ref(false)

async function runImportDefaultToc() {
  if (tocImportBusy.value) return
  tocImportBusy.value = true
  try {
    const res = await importDefaultTxtTocRules()
    ElMessage.success(`已导入 ${res.data?.count ?? 0} 条默认规则`)
    await loadTxtTocRules()
  } catch {
    // 已提示
  } finally {
    tocImportBusy.value = false
  }
}

onMounted(() => {
  loadTtsList()
  loadSysInfo()
  loadTxtTocRules()
})

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
          <span class="card-sub">HttpTTS 朗读源 · 已接入服务端（账号内多设备一致；服务不可用时降级本地）</span>
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

      <!-- txtTocRule（自定义 TXT 目录规则） -->
      <section class="card">
        <div class="card-head">
          <h2 class="card-title">txtTocRule</h2>
          <span class="card-sub">TXT 分章正则 · {{ customTocRules.length }} 条自定义</span>
          <button class="row-action" type="button" :disabled="tocImportBusy" @click="runImportDefaultToc">
            {{ tocImportBusy ? '导入中…' : '导入默认规则' }}
          </button>
          <button class="row-action" type="button" @click="openAddToc">新增规则</button>
        </div>
        <p class="card-note">上传 TXT 本地书时按启用的规则分章（无自定义规则时使用内置默认规则）。默认规则只读，可导入为自定义后编辑。</p>
        <p v-if="tocLoading" class="tts-empty">加载中…</p>
        <ul v-else-if="tocRules.length" class="tts-list toc-list">
          <li v-for="r in tocRules" :key="r.id" class="tts-row">
            <span class="tts-name" :title="r.name">{{ r.name }}</span>
            <span class="tts-url mono" :title="r.rule">{{ r.rule }}</span>
            <span class="tts-type">{{ r.id.startsWith('default-') ? '默认' : `#${r.serialNumber}` }}</span>
            <button
              class="switch"
              :class="{ on: r.enable }"
              type="button"
              role="switch"
              :aria-checked="r.enable"
              :title="r.id.startsWith('default-') ? '默认规则不可单独停用' : (r.enable ? '停用' : '启用')"
              @click="toggleTocRule(r)"
            >
              <span class="switch-knob"></span>
            </button>
            <button
              v-if="!r.id.startsWith('default-')"
              class="tts-del"
              type="button"
              title="删除规则"
              @click="askDeleteToc(r)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M4 7h16" />
                <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                <path d="M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4l.8-12" />
              </svg>
            </button>
          </li>
        </ul>
        <p v-else class="tts-empty">暂无自定义规则。可「导入默认规则」或新增正则（匹配行作为章节标题）。</p>
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
          <span class="row-value">v{{ sysInfo?.version || VERSION }}</span>
        </div>
        <template v-if="sysInfo">
          <div class="row">
            <span class="row-label">服务端口</span>
            <span class="row-value mono">{{ sysInfo.port }}</span>
          </div>
          <div class="row">
            <span class="row-label">用户数</span>
            <span class="row-value">{{ sysInfo.userCount }}</span>
          </div>
          <div class="row">
            <span class="row-label">书籍数</span>
            <span class="row-value">{{ sysInfo.bookCount }}</span>
          </div>
          <div class="row">
            <span class="row-label">书源数</span>
            <span class="row-value">{{ sysInfo.bookSourceCount }}</span>
          </div>
        </template>
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
              <p class="field-tip">听书源已接入服务端（POST /reader3/saveHttpTTS）；离线时降级本地存储</p>
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

    <!-- 新增 txtTocRule 弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="tocDialogOpen" class="dlg-overlay" @click.self="closeAddToc">
          <div class="dlg" role="dialog" aria-modal="true" aria-label="新增 txtTocRule" tabindex="-1" @keydown.esc="closeAddToc">
            <div class="dlg-head">
              <h2 class="dlg-title">新增 txtTocRule</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="tocBusy" @click="closeAddToc">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <form class="dlg-form" @submit.prevent="confirmAddToc">
              <label class="field">
                <span class="field-label">名称</span>
                <input v-model="tocForm.name" class="field-input" type="text" placeholder="留空则使用正则内容" maxlength="40" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">正则规则<em>*</em></span>
                <input v-model="tocForm.rule" class="field-input mono" type="text" placeholder="如 ^第.+章$" spellcheck="false" />
              </label>
              <div class="field">
                <span class="field-label">启用</span>
                <button
                  class="switch"
                  :class="{ on: tocForm.enable }"
                  type="button"
                  role="switch"
                  :aria-checked="tocForm.enable"
                  @click="tocForm.enable = !tocForm.enable"
                >
                  <span class="switch-knob"></span>
                </button>
              </div>
              <p class="field-tip">正则按行匹配（MULTILINE），匹配到的行作为章节标题；上传 TXT 时按启用的规则分章。</p>
              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="tocBusy" @click="closeAddToc">取消</button>
                <button class="accent-btn" type="submit" :disabled="tocBusy || !tocForm.rule.trim()">
                  {{ tocBusy ? '保存中…' : '保存' }}
                </button>
              </div>
            </form>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 删除 txtTocRule 确认弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="deletingToc" class="dlg-overlay" @click.self="closeDeleteToc">
          <div class="dlg dlg-confirm" role="alertdialog" aria-modal="true" aria-label="删除 txtTocRule" tabindex="-1" @keydown.esc="closeDeleteToc">
            <div class="dlg-head">
              <h2 class="dlg-title">删除规则</h2>
            </div>
            <p class="confirm-text">确定删除「{{ deletingToc.name }}」吗？此操作不可恢复。</p>
            <div class="dlg-actions">
              <button class="ghost-btn" type="button" :disabled="deleteTocBusy" @click="closeDeleteToc">取消</button>
              <button class="danger-btn" type="button" :disabled="deleteTocBusy" @click="confirmDeleteToc">
                {{ deleteTocBusy ? '删除中…' : '删除' }}
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

/* 极简开关（txtTocRule 启用切换） */
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
.toc-list .tts-name {
  max-width: 110px;
}
.toc-list .tts-url {
  font-size: 11.5px;
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
