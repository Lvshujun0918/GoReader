<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { deleteBookSource, getBookSources, getInvalidBookSources, saveBookSource, saveBookSources } from '@/api/sources'
import { deleteSourceSub, getSourceSubs, refreshSourceSub, saveSourceSub } from '@/api/sourceSubs'
import { exportBookSources } from '@/api/system'
import { bookSourceDebugSSE, type DebugAction } from '@/api/sourceDebug'
import { downloadBlob } from '@/utils/download'
import type { BookSource, SourceSub } from '@/types'

const router = useRouter()

/* ================= 列表 ================= */
const sources = ref<BookSource[]>([])
const loading = ref(true)
const errorMsg = ref('')

async function load() {
  loading.value = true
  errorMsg.value = ''
  try {
    const res = await getBookSources()
    sources.value = res.data ?? []
  } catch (err) {
    errorMsg.value = err instanceof Error ? err.message : '加载书源失败'
  } finally {
    loading.value = false
  }
}

/* ================= 分组筛选（细字胶囊） ================= */
const activeGroup = ref('全部')
const groups = computed(() => {
  const set = new Set<string>()
  for (const s of sources.value) {
    for (const g of (s.bookSourceGroup ?? '').split(/\s+/)) {
      if (g) set.add(g)
    }
  }
  return Array.from(set).sort()
})

/* ================= 搜索过滤 ================= */
const filterKey = ref('')
const filtered = computed(() => {
  const kw = filterKey.value.trim().toLowerCase()
  return sources.value.filter((s) => {
    if (activeGroup.value !== '全部') {
      const gs = (s.bookSourceGroup ?? '').split(/\s+/)
      if (!gs.includes(activeGroup.value)) return false
    }
    if (!kw) return true
    return (
      s.bookSourceName.toLowerCase().includes(kw) ||
      s.bookSourceUrl.toLowerCase().includes(kw) ||
      (s.bookSourceGroup ?? '').toLowerCase().includes(kw)
    )
  })
})

const enabledCount = computed(() => sources.value.filter((s) => s.enabled).length)

/** 判断是否接口未实现（404 / 后端未就绪） */
function isNotImplemented(err: unknown): boolean {
  const e = err as { response?: { status?: number }; message?: string } | null | undefined
  const status = e?.response?.status
  if (status === 404 || status === 501) return true
  const msg = e?.message ?? ''
  return !e?.response && (msg.includes('404') || msg.includes('Network Error'))
}

/* ================= 失效检测（GET /reader3/getInvalidBookSources） ================= */

const invalidChecking = ref(false)
const invalidSources = ref<Set<string>>(new Set())
const invalidMsg = ref('')
const invalidMsgError = ref(false)

/** 归一化后端返回：string[] 或含 bookSourceUrl 的对象数组 */
function normalizeInvalid(raw: unknown): string[] {
  if (!Array.isArray(raw)) return []
  const out: string[] = []
  for (const item of raw) {
    if (typeof item === 'string') out.push(item)
    else if (item && typeof item === 'object') {
      const u = (item as Record<string, unknown>).bookSourceUrl
      if (typeof u === 'string') out.push(u)
    }
  }
  return out
}

async function checkInvalid() {
  if (invalidChecking.value) return
  invalidChecking.value = true
  invalidMsg.value = ''
  invalidMsgError.value = false
  try {
    const res = await getInvalidBookSources()
    invalidSources.value = new Set(normalizeInvalid(res.data))
    const n = invalidSources.value.size
    invalidMsg.value = n === 0 ? '检测完成：未发现失效书源' : `检测完成：发现 ${n} 个失效书源（已红色标记）`
  } catch (err) {
    invalidMsg.value = isNotImplemented(err)
      ? '失效检测接口后端暂未提供（GET /reader3/getInvalidBookSources）'
      : `检测失败：${err instanceof Error ? err.message : '请稍后重试'}`
    invalidMsgError.value = true
  } finally {
    invalidChecking.value = false
  }
}

/* ================= 书源调试（GET /reader3/bookSourceDebugSSE：SSE 逐步日志） ================= */

const DEBUG_ACTIONS: { value: DebugAction; label: string; tip: string; needKey: boolean; needUrl: boolean }[] = [
  { value: 'search', label: '搜索', tip: '关键词搜索', needKey: true, needUrl: false },
  { value: 'toc', label: '目录', tip: '获取章节目录', needKey: false, needUrl: true },
  { value: 'content', label: '正文', tip: '获取章节正文', needKey: false, needUrl: true },
]

const debugOpen = ref(false)
const debugSource = ref<BookSource | null>(null)
const debugAction = ref<DebugAction>('search')
const debugInput = ref('')
const debugRunning = ref(false)
const debugLogs = ref<{ text: string; error: boolean }[]>([])
const debugMsg = ref('')
const debugMsgError = ref(false)
let debugHandle: { close: () => void } | null = null

function debugActionMeta(a: DebugAction) {
  return DEBUG_ACTIONS.find((x) => x.value === a) ?? DEBUG_ACTIONS[0]
}

const debugPlaceholder = computed(() => {
  if (debugAction.value === 'search') return '搜索关键词，如：斗破苍穹'
  if (debugAction.value === 'toc') return '书籍 URL（目录页地址，可留空）'
  return '章节 URL'
})

const debugTip = computed(() => {
  const meta = debugActionMeta(debugAction.value)
  return `动作：${meta.label}（${meta.tip}）· 步骤经 SSE 实时输出`
})

const debugCanRun = computed(() => {
  const meta = debugActionMeta(debugAction.value)
  return !meta.needKey || debugInput.value.trim().length > 0
})

function openDebug(s: BookSource) {
  debugSource.value = s
  debugAction.value = 'search'
  debugInput.value = ''
  debugLogs.value = []
  debugMsg.value = ''
  debugMsgError.value = false
  debugOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeDebug() {
  if (debugRunning.value) return
  debugHandle?.close()
  debugHandle = null
  debugOpen.value = false
  document.body.style.overflow = ''
}

function pushLog(text: string, error = false) {
  debugLogs.value.push({ text, error })
}

/** 运行调试：建立 SSE 连接，逐步追加日志；失败红色标记 */
async function runDebug() {
  const s = debugSource.value
  if (!s || debugRunning.value) return
  const meta = debugActionMeta(debugAction.value)
  const input = debugInput.value.trim()
  if (meta.needKey && !input) {
    debugMsg.value = '请输入搜索关键词'
    debugMsgError.value = true
    return
  }
  debugRunning.value = true
  debugMsg.value = ''
  debugMsgError.value = false
  debugLogs.value = []
  pushLog(`开始调试「${s.bookSourceName}」· ${meta.label}`)
  try {
    const handle = await bookSourceDebugSSE(
      {
        bookSourceUrl: s.bookSourceUrl,
        action: debugAction.value,
        key: meta.needKey ? input : undefined,
        chapterUrl: meta.needUrl && input ? input : undefined,
      },
      {
        onStep: (message) => pushLog(message),
        onResult: (data) => {
          let summary: string
          try {
            summary = typeof data === 'string' ? data : JSON.stringify(data, null, 2)
          } catch {
            summary = String(data)
          }
          pushLog(`结果：${summary}`)
          debugMsg.value = '调试完成'
          debugMsgError.value = false
        },
        onEnd: () => {
          debugRunning.value = false
          debugMsg.value = debugMsg.value || '调试完成'
        },
        onStreamError: (msg) => {
          debugRunning.value = false
          debugMsg.value = `调试失败：${msg}`
          debugMsgError.value = true
          pushLog(`错误：${msg}`, true)
        },
      },
    )
    debugHandle = handle
  } catch {
    debugRunning.value = false
    debugMsg.value = '调试接口后端暂未提供（GET /reader3/bookSourceDebugSSE）'
    debugMsgError.value = true
    pushLog('连接失败：后端未就绪或网络异常', true)
  }
}

function stopDebug() {
  debugHandle?.close()
  debugHandle = null
  debugRunning.value = false
  debugMsg.value = '已停止调试'
  debugMsgError.value = false
}

/* 切换动作时清空输入与日志 */
watch(debugAction, () => {
  if (debugRunning.value) return
  debugInput.value = ''
  debugLogs.value = []
  debugMsg.value = ''
  debugMsgError.value = false
})

/* ================= 启用开关 ================= */
const toggling = ref<Set<string>>(new Set())

async function toggleSource(s: BookSource) {
  if (toggling.value.has(s.bookSourceUrl)) return
  toggling.value.add(s.bookSourceUrl)
  const prev = s.enabled
  s.enabled = !prev // 乐观更新
  try {
    await saveBookSource({ ...s, enabled: !prev })
  } catch {
    s.enabled = prev // 失败回滚（错误提示由拦截器处理）
  } finally {
    toggling.value.delete(s.bookSourceUrl)
  }
}

/* ================= 删除（极简确认弹窗） ================= */
const deleting = ref<BookSource | null>(null)
const deleteBusy = ref(false)

function askDelete(s: BookSource) {
  deleting.value = s
  document.body.style.overflow = 'hidden'
}

async function confirmDelete() {
  const s = deleting.value
  if (!s || deleteBusy.value) return
  deleteBusy.value = true
  try {
    await deleteBookSource(s.bookSourceUrl)
    sources.value = sources.value.filter((x) => x.bookSourceUrl !== s.bookSourceUrl)
    closeDelete()
  } catch {
    // 错误提示已由拦截器处理
  } finally {
    deleteBusy.value = false
  }
}

function closeDelete() {
  deleting.value = null
  document.body.style.overflow = ''
}

/* ================= 新增（极简弹窗表单） ================= */
const addOpen = ref(false)
const addBusy = ref(false)
const addForm = ref({ bookSourceUrl: '', bookSourceName: '', bookSourceGroup: '' })

function openAdd() {
  addForm.value = { bookSourceUrl: '', bookSourceName: '', bookSourceGroup: '' }
  addOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeAdd() {
  if (addBusy.value) return
  addOpen.value = false
  document.body.style.overflow = ''
}

/** 后端要求完整 BookSource：补默认值 */
function buildSource(form: { bookSourceUrl: string; bookSourceName: string; bookSourceGroup: string }): BookSource {
  return {
    bookSourceUrl: form.bookSourceUrl.trim(),
    bookSourceName: form.bookSourceName.trim() || form.bookSourceUrl.trim(),
    bookSourceGroup: form.bookSourceGroup.trim() || null,
    bookSourceType: 0,
    customOrder: 0,
    enabled: true,
    enabledExplore: false,
    lastUpdateTime: 0,
    respondTime: 0,
    weight: 0,
  }
}

async function confirmAdd() {
  if (addBusy.value) return
  const url = addForm.value.bookSourceUrl.trim()
  if (!url) return
  addBusy.value = true
  try {
    await saveBookSource(buildSource(addForm.value))
    closeAdd()
    await load()
  } catch {
    // 错误提示已由拦截器处理
  } finally {
    addBusy.value = false
  }
}

/* ================= 导出（blob 下载 bookSource.json） ================= */
const exporting = ref(false)

async function doExport() {
  if (exporting.value) return
  exporting.value = true
  try {
    const blob = await exportBookSources()
    await downloadBlob(blob, 'bookSource.json')
  } catch {
    // 请求层已提示
  } finally {
    exporting.value = false
  }
}

/* ================= 本地文件导入（input file → 解析 JSON → saveBookSources） ================= */
const localFileInput = ref<HTMLInputElement | null>(null)
const localImportBusy = ref(false)

function openLocalImport() {
  localFileInput.value?.click()
}

async function onLocalFilePick(e: Event) {
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 允许再次选择同一文件
  if (!file || localImportBusy.value) return
  localImportBusy.value = true
  try {
    const raw: unknown = JSON.parse(await file.text())
    const list = normalizeSources(raw)
    if (list.length === 0) {
      ElMessage.warning('未识别到书源（需为书源数组或含 bookSourceList 的对象）')
      return
    }
    const res = await saveBookSources(list)
    ElMessage.success(`成功导入 ${res.data?.count ?? list.length} 个书源`)
    await load()
  } catch (err) {
    if (err instanceof SyntaxError) {
      ElMessage.error('文件不是有效的 JSON')
    }
    // 其余错误（网络/后端失败）已由请求拦截器提示
  } finally {
    localImportBusy.value = false
  }
}

/* ================= 远程导入（fetch JSON → 批量 saveBookSources） ================= */
const importOpen = ref(false)
const importBusy = ref(false)
const importUrl = ref('')
const importTip = ref('')

function openImport() {
  importUrl.value = ''
  importTip.value = ''
  importOpen.value = true
  document.body.style.overflow = 'hidden'
}

function closeImport() {
  if (importBusy.value) return
  importOpen.value = false
  document.body.style.overflow = ''
}

/** 兼容三种格式：数组 / {bookSourceList:[...]} / 单个书源对象；缺失必填字段补默认 */
function normalizeSources(raw: unknown): BookSource[] {
  let arr: unknown[] = []
  if (Array.isArray(raw)) {
    arr = raw
  } else if (raw && typeof raw === 'object') {
    const obj = raw as Record<string, unknown>
    if (Array.isArray(obj.bookSourceList)) arr = obj.bookSourceList
    else if (obj.bookSourceUrl) arr = [obj]
  }
  const out: BookSource[] = []
  for (const item of arr) {
    if (!item || typeof item !== 'object') continue
    const s = item as Record<string, unknown>
    const url = typeof s.bookSourceUrl === 'string' ? s.bookSourceUrl.trim() : ''
    if (!url) continue
    out.push({
      ...(s as unknown as BookSource),
      bookSourceUrl: url,
      bookSourceName:
        typeof s.bookSourceName === 'string' && s.bookSourceName.trim()
          ? s.bookSourceName.trim()
          : url,
      bookSourceType: typeof s.bookSourceType === 'number' ? s.bookSourceType : 0,
      customOrder: typeof s.customOrder === 'number' ? s.customOrder : 0,
      enabled: typeof s.enabled === 'boolean' ? s.enabled : true,
      enabledExplore: typeof s.enabledExplore === 'boolean' ? s.enabledExplore : false,
      lastUpdateTime: typeof s.lastUpdateTime === 'number' ? s.lastUpdateTime : 0,
      respondTime: typeof s.respondTime === 'number' ? s.respondTime : 0,
      weight: typeof s.weight === 'number' ? s.weight : 0,
    })
  }
  return out
}

async function confirmImport() {
  if (importBusy.value) return
  const url = importUrl.value.trim()
  if (!url) return
  importBusy.value = true
  importTip.value = ''
  try {
    const resp = await fetch(url, { mode: 'cors' })
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
    const raw: unknown = await resp.json()
    const list = normalizeSources(raw)
    if (list.length === 0) {
      importTip.value = '未识别到书源（需为书源数组或含 bookSourceList 的对象）'
      return
    }
    const res = await saveBookSources(list)
    const count = res.data?.count ?? list.length
    importTip.value = `成功导入 ${count} 个书源`
    closeImport()
    await load()
  } catch (err) {
    importTip.value =
      err instanceof Error && err.message
        ? `导入失败：${err.message}（若为浏览器跨域限制，可下载后手动新增）`
        : '导入失败，请检查地址'
  } finally {
    importBusy.value = false
  }
}

/* ================= 订阅源（远程书源订阅，后端 /reader3/getSourceSubs 等为主，localStorage 降级，见 api/sourceSubs.ts） ================= */
const subs = ref<SourceSub[]>([])
const subUrl = ref('')
const subBusy = ref(false)
const subBusyUrls = ref<Set<string>>(new Set())
const subMsg = ref('')
const subMsgError = ref(false)

function setSubMsg(msg: string, isError = false) {
  subMsg.value = msg
  subMsgError.value = isError
}

/** 订阅显示名：优先响应里的 name / bookSourceGroup，否则取域名 */
function subNameFromRaw(raw: unknown, url: string): string {
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    const obj = raw as Record<string, unknown>
    if (typeof obj.name === 'string' && obj.name.trim()) return obj.name.trim()
    if (typeof obj.bookSourceGroup === 'string' && obj.bookSourceGroup.trim()) return obj.bookSourceGroup.trim()
  }
  try {
    return new URL(url).hostname
  } catch {
    return url
  }
}

/** 拉取远程书源 JSON 并批量导入，返回导入数量 */
async function fetchAndImport(url: string): Promise<number> {
  const resp = await fetch(url, { mode: 'cors' })
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
  const raw: unknown = await resp.json()
  const list = normalizeSources(raw)
  if (list.length === 0) throw new Error('未识别到书源（需为书源数组或含 bookSourceList 的对象）')
  const res = await saveBookSources(list)
  return res.data?.count ?? list.length
}

/**
 * 刷新订阅并导入书源：后端 POST /reader3/refreshSourceSub 优先（服务端拉取远程 JSON 并导入书源表）；
 * 后端不可用时降级为前端 fetch + saveBookSources（preFetched 可复用已拉取的列表，避免二次请求）。
 */
async function refreshAndImport(url: string, preFetched?: BookSource[]): Promise<number> {
  const res = await refreshSourceSub(url)
  if (res.isSuccess) return res.data?.count ?? preFetched?.length ?? 0
  if (preFetched) {
    const saveRes = await saveBookSources(preFetched)
    return saveRes.data?.count ?? preFetched.length
  }
  return fetchAndImport(url)
}

/**
 * 注册订阅并导入书源：后端 POST /reader3/saveSourceSub 优先（服务端抓取校验 + 订阅入库 + 批量导入，返回导入数）；
 * 降级（后端不可达，data=null）：写入 localStorage 后由前端导入（preFetched 可复用已拉取的列表）。
 */
async function registerAndImport(url: string, name: string, preFetched?: BookSource[]): Promise<number> {
  const res = await saveSourceSub(url, name)
  if (res.data && typeof res.data.count === 'number') return res.data.count
  if (preFetched) {
    const saveRes = await saveBookSources(preFetched)
    return saveRes.data?.count ?? preFetched.length
  }
  return fetchAndImport(url)
}

/** 新增订阅：拉取书源数组取名称 → 注册订阅（后端 saveSourceSub 抓取+导入，降级 localStorage+前端导入） */
async function confirmAddSub() {
  if (subBusy.value) return
  const url = subUrl.value.trim()
  if (!url) return
  subBusy.value = true
  setSubMsg('')
  try {
    // ① 前端拉取一次：校验格式 + 提取订阅名（后端契约 saveSourceSub 需 name）
    const resp = await fetch(url, { mode: 'cors' })
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
    const raw: unknown = await resp.json()
    const list = normalizeSources(raw)
    if (list.length === 0) throw new Error('未识别到书源（需为书源数组或含 bookSourceList 的对象）')
    const name = subNameFromRaw(raw, url)
    // ② 注册订阅 + 导入书源（后端优先；降级用已拉取的列表导入）
    const count = await registerAndImport(url, name, list)
    const existing = subs.value.find((x) => x.url === url)
    if (existing) {
      existing.name = name
      existing.enabled = true
    } else {
      subs.value.push({ url, name, enabled: true })
    }
    subUrl.value = ''
    setSubMsg(`订阅成功：已导入 ${count} 个书源`)
    await load() // 刷新书源列表
  } catch (err) {
    setSubMsg(
      `订阅失败：${err instanceof Error && err.message ? err.message : '未知错误'}（若为浏览器跨域限制，可下载后手动新增）`,
      true,
    )
  } finally {
    subBusy.value = false
  }
}

/** 启用/停用订阅：启用时注册订阅并重新导入（后端 saveSourceSub / 降级前端导入）；停用仅改本地记录（已导入书源保留） */
async function toggleSub(sub: SourceSub) {
  if (subBusyUrls.value.has(sub.url)) return
  const prev = sub.enabled
  subBusyUrls.value.add(sub.url)
  try {
    if (!prev) {
      const count = await registerAndImport(sub.url, sub.name) // 注册订阅（幂等；后端优先，失败降级 localStorage）
      sub.enabled = true
      setSubMsg(`已启用「${sub.name}」，重新导入 ${count} 个书源`)
      await load()
    } else {
      sub.enabled = false
      setSubMsg('已停用订阅（已导入的书源保留）')
    }
  } catch (err) {
    setSubMsg(
      `导入失败：${err instanceof Error && err.message ? err.message : '未知错误'}（订阅未启用）`,
      true,
    )
  } finally {
    subBusyUrls.value.delete(sub.url)
  }
}

/** 刷新订阅：重新拉取远程书源并批量导入（后端 refreshSourceSub / 降级前端导入） */
async function refreshSub(sub: SourceSub) {
  if (subBusyUrls.value.has(sub.url)) return
  subBusyUrls.value.add(sub.url)
  try {
    const count = await refreshAndImport(sub.url)
    setSubMsg(`已刷新「${sub.name}」，导入 ${count} 个书源`)
    await load()
  } catch (err) {
    setSubMsg(
      `刷新失败：${err instanceof Error && err.message ? err.message : '未知错误'}（若为浏览器跨域限制，可下载后手动新增）`,
      true,
    )
  } finally {
    subBusyUrls.value.delete(sub.url)
  }
}

/* 删除订阅（后端优先；降级删除本地记录） */
const deletingSub = ref<SourceSub | null>(null)
const deleteSubBusy = ref(false)

function askDeleteSub(sub: SourceSub) {
  deletingSub.value = sub
  document.body.style.overflow = 'hidden'
}

async function confirmDeleteSub() {
  const s = deletingSub.value
  if (!s || deleteSubBusy.value) return
  deleteSubBusy.value = true
  try {
    await deleteSourceSub(s.url)
    subs.value = subs.value.filter((x) => x.url !== s.url)
    setSubMsg('已删除订阅记录（已导入的书源保留）')
    closeDeleteSub()
  } catch {
    // 已提示
  } finally {
    deleteSubBusy.value = false
  }
}

function closeDeleteSub() {
  deletingSub.value = null
  document.body.style.overflow = ''
}

async function loadSubs() {
  const res = await getSourceSubs() // 后端优先；失败降级 localStorage（api 层已处理）
  subs.value = res.data ?? []
}

onMounted(() => {
  load()
  void loadSubs()
})
</script>

<template>
  <div class="sources-page">
    <!-- 极简顶栏：返回书架 -->
    <header class="topbar">
      <button class="back-btn" type="button" @click="router.push('/')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5" />
          <path d="M11 18l-6-6 6-6" />
        </svg>
        <span>书架</span>
      </button>
      <span class="brand">夜读<span class="brand-dot">.</span></span>
    </header>

    <main class="content">
      <div class="section-head">
        <h1 class="page-title">书源管理</h1>
        <span class="count">{{ sources.length }} 个 · {{ enabledCount }} 启用</span>
        <div class="head-actions">
          <button class="ghost-btn" type="button" :disabled="localImportBusy" @click="openLocalImport">
            {{ localImportBusy ? '导入中…' : '本地导入' }}
          </button>
          <button class="ghost-btn" type="button" @click="openImport">远程导入</button>
          <button
            class="ghost-btn"
            type="button"
            :disabled="invalidChecking"
            title="检测失效书源（GET /reader3/getInvalidBookSources）"
            @click="checkInvalid"
          >
            {{ invalidChecking ? '检测中…' : '检测失效' }}
          </button>
          <button
            class="ghost-btn"
            type="button"
            :disabled="exporting"
            title="下载当前账号全部书源（bookSource.json）"
            @click="doExport"
          >
            {{ exporting ? '导出中…' : '导出' }}
          </button>
          <button class="accent-outline-btn" type="button" @click="openAdd">新增书源</button>
          <input
            ref="localFileInput"
            class="local-file-input"
            type="file"
            accept=".json,application/json"
            @change="onLocalFilePick"
          />
        </div>
      </div>

      <!-- 分组筛选（细字胶囊）+ 搜索过滤 -->
      <div class="filter-row">
        <div class="group-capsules">
          <button
            class="capsule"
            :class="{ active: activeGroup === '全部' }"
            type="button"
            @click="activeGroup = '全部'"
          >
            全部
          </button>
          <button
            v-for="g in groups"
            :key="g"
            class="capsule"
            :class="{ active: activeGroup === g }"
            type="button"
            @click="activeGroup = g"
          >
            {{ g }}
          </button>
        </div>
        <div class="filter-box">
          <svg class="filter-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
            <circle cx="11" cy="11" r="6.5" />
            <path d="M20 20l-3.8-3.8" />
          </svg>
          <input
            v-model="filterKey"
            class="filter-input"
            type="text"
            placeholder="筛选名称 / 地址"
            spellcheck="false"
          />
        </div>
      </div>

      <!-- 失效检测结果提示 -->
      <p v-if="invalidMsg" class="invalid-note" :class="{ error: invalidMsgError }">{{ invalidMsg }}</p>

      <!-- 加载态 -->
      <div v-if="loading" class="state-row">
        <svg class="mini-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
          <path d="M21 12a9 9 0 1 1-6.2-8.56" />
        </svg>
        <span class="state-text">加载中…</span>
      </div>

      <!-- 错误态 -->
      <div v-else-if="errorMsg" class="state-row">
        <span class="state-text error">{{ errorMsg }}</span>
        <button class="retry-btn" type="button" @click="load">重试</button>
      </div>

      <!-- 空状态 -->
      <div v-else-if="filtered.length === 0" class="state-row">
        <span class="state-text">
          {{ sources.length === 0 ? '暂无书源，点击右上角新增或远程导入' : '没有匹配的书源' }}
        </span>
      </div>

      <!-- 书源列表 -->
      <ul v-else class="source-list">
        <li v-for="s in filtered" :key="s.bookSourceUrl" class="source-row" :class="{ invalid: invalidSources.has(s.bookSourceUrl) }">
          <div class="source-main">
            <p class="source-name" :title="s.bookSourceName">{{ s.bookSourceName }}</p>
            <p class="source-url" :title="s.bookSourceUrl">{{ s.bookSourceUrl }}</p>
          </div>
          <span v-if="s.bookSourceGroup" class="source-group" :title="s.bookSourceGroup">
            {{ s.bookSourceGroup }}
          </span>
          <span v-if="invalidSources.has(s.bookSourceUrl)" class="source-badge invalid">失效</span>
          <span class="source-state" :class="{ on: s.enabled }">{{ s.enabled ? '启用' : '停用' }}</span>
          <button
            class="test-btn"
            type="button"
            title="调试书源（搜索 / 目录 / 正文，SSE 逐步日志）"
            @click="openDebug(s)"
          >
            测试
          </button>
          <button
            class="switch"
            :class="{ on: s.enabled }"
            type="button"
            role="switch"
            :aria-checked="s.enabled"
            :title="s.enabled ? '停用' : '启用'"
            @click="toggleSource(s)"
          >
            <span class="switch-knob"></span>
          </button>
          <button class="delete-btn" type="button" title="删除书源" @click="askDelete(s)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
              <path d="M4 7h16" />
              <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
              <path d="M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13" />
            </svg>
          </button>
        </li>
      </ul>

      <!-- 订阅源：远程书源订阅（后端 /reader3/getSourceSubs 等为主，localStorage 降级，见 api/sourceSubs.ts） -->
      <section class="subs-section">
        <div class="subs-head">
          <h2 class="subs-title">订阅源</h2>
          <span class="subs-sub">远程书源订阅 · 已接入服务端（账号内多设备一致；服务不可用时降级本地存储）</span>
        </div>
        <form class="subs-add" @submit.prevent="confirmAddSub">
          <input
            v-model="subUrl"
            class="filter-input subs-input"
            type="text"
            placeholder="订阅书源 JSON 地址，如 https://…/bookSource.json"
            spellcheck="false"
          />
          <button class="accent-outline-btn" type="submit" :disabled="subBusy || !subUrl.trim()">
            {{ subBusy ? '订阅中…' : '订阅' }}
          </button>
        </form>
        <p v-if="subMsg" class="subs-msg" :class="{ error: subMsgError }">{{ subMsg }}</p>
        <p v-if="subs.length === 0" class="subs-empty">暂无订阅。订阅后书源将批量导入，启用开关可随时重新导入。</p>
        <ul v-else class="subs-list">
          <li v-for="sub in subs" :key="sub.url" class="subs-row">
            <div class="subs-main">
              <p class="subs-name" :title="sub.name">{{ sub.name }}</p>
              <p class="subs-url" :title="sub.url">{{ sub.url }}</p>
            </div>
            <span class="source-state" :class="{ on: sub.enabled }">{{ sub.enabled ? '启用' : '停用' }}</span>
            <button
              class="switch"
              :class="{ on: sub.enabled }"
              type="button"
              role="switch"
              :aria-checked="sub.enabled"
              :title="sub.enabled ? '停用订阅（已导入的书源保留）' : '启用并重新导入书源'"
              @click="toggleSub(sub)"
            >
              <span class="switch-knob"></span>
            </button>
            <button
              class="refresh-btn"
              type="button"
              title="刷新订阅（重新拉取并导入书源）"
              :disabled="subBusyUrls.has(sub.url)"
              @click="refreshSub(sub)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M20 11a8 8 0 1 0-2.3 6.3" />
                <path d="M20 5v6h-6" />
              </svg>
            </button>
            <button class="delete-btn" type="button" title="删除订阅（仅本地记录）" @click="askDeleteSub(sub)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M4 7h16" />
                <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
                <path d="M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13" />
              </svg>
            </button>
          </li>
        </ul>
      </section>
    </main>

    <!-- 新增书源弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="addOpen" class="dlg-overlay" @click.self="closeAdd">
          <div class="dlg" role="dialog" aria-modal="true" aria-label="新增书源" tabindex="-1" @keydown.esc="closeAdd">
            <div class="dlg-head">
              <h2 class="dlg-title">新增书源</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="addBusy" @click="closeAdd">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <form class="dlg-form" @submit.prevent="confirmAdd">
              <label class="field">
                <span class="field-label">URL<em>*</em></span>
                <input v-model="addForm.bookSourceUrl" class="field-input" type="text" placeholder="https://example.com" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">名称</span>
                <input v-model="addForm.bookSourceName" class="field-input" type="text" placeholder="留空则使用 URL" spellcheck="false" />
              </label>
              <label class="field">
                <span class="field-label">分组</span>
                <input v-model="addForm.bookSourceGroup" class="field-input" type="text" placeholder="可留空，多个分组用空格分隔" spellcheck="false" />
              </label>
              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="addBusy" @click="closeAdd">取消</button>
                <button class="accent-btn" type="submit" :disabled="addBusy || !addForm.bookSourceUrl.trim()">
                  {{ addBusy ? '保存中…' : '保存' }}
                </button>
              </div>
            </form>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 远程导入弹窗 -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="importOpen" class="dlg-overlay" @click.self="closeImport">
          <div class="dlg" role="dialog" aria-modal="true" aria-label="远程导入书源" tabindex="-1" @keydown.esc="closeImport">
            <div class="dlg-head">
              <h2 class="dlg-title">远程导入书源</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="importBusy" @click="closeImport">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <form class="dlg-form" @submit.prevent="confirmImport">
              <label class="field">
                <span class="field-label">书源 JSON 地址<em>*</em></span>
                <input v-model="importUrl" class="field-input" type="text" placeholder="https://…/bookSource.json" spellcheck="false" />
              </label>
              <p class="field-tip">支持书源数组 / {bookSourceList: [...]} / 单个书源对象</p>
              <p v-if="importTip" class="field-tip" :class="{ error: importBusy }">{{ importTip }}</p>
              <div class="dlg-actions">
                <button class="ghost-btn" type="button" :disabled="importBusy" @click="closeImport">取消</button>
                <button class="accent-btn" type="submit" :disabled="importBusy || !importUrl.trim()">
                  {{ importBusy ? '导入中…' : '导入' }}
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
          <div class="dlg dlg-confirm" role="alertdialog" aria-modal="true" aria-label="删除书源" tabindex="-1" @keydown.esc="closeDelete">
            <div class="dlg-head">
              <h2 class="dlg-title">删除书源</h2>
            </div>
            <p class="confirm-text">
              确定删除「{{ deleting.bookSourceName }}」吗？此操作不可恢复。
            </p>
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

    <!-- 删除订阅确认弹窗（极简） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="deletingSub" class="dlg-overlay" @click.self="closeDeleteSub">
          <div class="dlg dlg-confirm" role="alertdialog" aria-modal="true" aria-label="删除订阅" tabindex="-1" @keydown.esc="closeDeleteSub">
            <div class="dlg-head">
              <h2 class="dlg-title">删除订阅</h2>
            </div>
            <p class="confirm-text">
              确定删除订阅「{{ deletingSub.name }}」吗？仅删除订阅记录，已导入的书源不受影响。
            </p>
            <div class="dlg-actions">
              <button class="ghost-btn" type="button" :disabled="deleteSubBusy" @click="closeDeleteSub">取消</button>
              <button class="danger-btn" type="button" :disabled="deleteSubBusy" @click="confirmDeleteSub">
                {{ deleteSubBusy ? '删除中…' : '删除' }}
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
    <!-- 书源调试弹窗（GET /reader3/bookSourceDebugSSE：动作选择 + 输入 + SSE 逐步日志） -->
    <Teleport to="body">
      <Transition name="dlg">
        <div v-if="debugOpen" class="dlg-overlay" @click.self="closeDebug">
          <div
            class="dlg dlg-debug"
            role="dialog"
            aria-modal="true"
            aria-label="书源调试"
            tabindex="-1"
            @keydown.esc="closeDebug"
          >
            <div class="dlg-head">
              <h2 class="dlg-title">调试 · {{ debugSource?.bookSourceName }}</h2>
              <button class="dlg-close" type="button" title="关闭" :disabled="debugRunning" @click="closeDebug">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </button>
            </div>
            <div class="debug-actions">
              <button
                v-for="a in DEBUG_ACTIONS"
                :key="a.value"
                class="capsule debug-act"
                :class="{ active: debugAction === a.value }"
                type="button"
                :disabled="debugRunning"
                @click="debugAction = a.value"
              >
                {{ a.label }}
              </button>
            </div>
            <input
              v-model="debugInput"
              class="debug-input"
              type="text"
              :placeholder="debugPlaceholder"
              spellcheck="false"
              :disabled="debugRunning"
              @keydown.enter="runDebug"
            />
            <p class="field-tip">{{ debugTip }}</p>
            <div class="debug-log">
              <p v-for="(l, i) in debugLogs" :key="i" class="debug-line" :class="{ error: l.error }">
                {{ l.text }}
              </p>
              <p v-if="debugRunning" class="debug-line running">… 执行中（逐步输出）</p>
            </div>
            <p v-if="debugMsg" class="debug-msg" :class="{ error: debugMsgError }">{{ debugMsg }}</p>
            <div class="dlg-actions">
              <button v-if="debugRunning" class="ghost-btn" type="button" @click="stopDebug">停止</button>
              <template v-else>
                <button class="ghost-btn" type="button" @click="closeDebug">关闭</button>
                <button class="accent-btn" type="button" :disabled="!debugCanRun" @click="runDebug">开始调试</button>
              </template>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.sources-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  animation: fade-in 0.2s ease both;
}

/* ================= 顶栏 ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 14px 32px;
  background: rgba(250, 250, 250, 0.86);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}
.back-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 13px;
  font-weight: 300;
  letter-spacing: 1px;
  cursor: pointer;
  transition: color 0.2s ease;
}
.back-btn:hover {
  color: var(--accent);
}
.back-btn svg {
  width: 14px;
  height: 14px;
}
.brand {
  font-size: 15px;
  font-weight: 300;
  letter-spacing: 3px;
  color: var(--text-1);
}
.brand-dot {
  color: var(--accent);
  font-weight: 400;
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
.page-title {
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
.head-actions {
  margin-left: auto;
  display: flex;
  gap: 8px;
}
.local-file-input {
  display: none;
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
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease;
}
.ghost-btn:hover:not(:disabled) {
  color: var(--text-1);
  border-color: var(--border-strong);
}
.accent-outline-btn {
  padding: 7px 16px;
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
.accent-outline-btn:hover:not(:disabled) {
  color: var(--accent-deep);
  border-color: var(--accent-deep);
  background: var(--accent-soft);
}
.ghost-btn:disabled,
.accent-outline-btn:disabled,
.accent-btn:disabled,
.danger-btn:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}

/* ================= 筛选行 ================= */
.filter-row {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 20px;
}
.group-capsules {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.capsule {
  padding: 4px 13px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 300;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.capsule:hover {
  color: var(--accent);
  border-color: var(--accent);
}
.capsule.active {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
  font-weight: 400;
}
.filter-box {
  position: relative;
  flex-shrink: 0;
  width: 200px;
}
.filter-icon {
  position: absolute;
  left: 10px;
  top: 50%;
  transform: translateY(-50%);
  width: 13px;
  height: 13px;
  color: var(--text-3);
  pointer-events: none;
  transition: color 0.2s ease;
}
.filter-box:focus-within .filter-icon {
  color: var(--accent);
}
.filter-input {
  width: 100%;
  height: 34px;
  padding: 0 12px 0 30px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-1);
  font-family: inherit;
  font-size: 12.5px;
  font-weight: 400;
  outline: none;
  transition: border-color 0.2s ease;
}
.filter-input::placeholder {
  color: var(--text-3);
  font-weight: 300;
}
.filter-input:focus {
  border-color: var(--accent);
}

/* ================= 状态行 ================= */
.state-row {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 72px 0;
}
.state-text {
  font-size: 13.5px;
  font-weight: 300;
  letter-spacing: 1px;
  color: var(--text-3);
}
.state-text.error {
  color: #cf4444;
}
.mini-spin {
  width: 13px;
  height: 13px;
  color: var(--accent);
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
.retry-btn {
  padding: 5px 14px;
  border-radius: var(--radius);
  border: 1px solid var(--border-strong);
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
.retry-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
}

/* ================= 书源列表 ================= */
.source-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}
.source-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 13px 6px;
  border-bottom: 1px solid var(--border);
}
.source-row:first-child {
  border-top: 1px solid var(--border);
}
.source-main {
  flex: 1;
  min-width: 0;
}
.source-name {
  margin: 0;
  font-size: 13.5px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.source-url {
  margin: 3px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.source-group {
  flex-shrink: 0;
  max-width: 140px;
  padding: 1px 8px;
  border-radius: 4px;
  border: 1px solid var(--border);
  color: var(--text-3);
  font-size: 11px;
  font-weight: 300;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.source-state {
  flex-shrink: 0;
  width: 30px;
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.source-state.on {
  color: var(--accent);
  font-weight: 400;
}

/* 极简开关：细线圆角条 */
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

.delete-btn {
  flex-shrink: 0;
  width: 28px;
  height: 28px;
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
.delete-btn:hover {
  color: #cf4444;
  background: rgba(207, 68, 68, 0.06);
}
.delete-btn svg {
  width: 13px;
  height: 13px;
}

/* 刷新订阅按钮（细字图标） */
.refresh-btn {
  flex-shrink: 0;
  width: 28px;
  height: 28px;
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
.refresh-btn:hover:not(:disabled) {
  color: var(--accent);
  background: rgba(64, 158, 120, 0.06);
}
.refresh-btn:disabled {
  cursor: not-allowed;
  opacity: 0.4;
}
.refresh-btn svg {
  width: 13px;
  height: 13px;
}

/* ================= 失效检测 ================= */
.invalid-note {
  margin: 0 0 14px;
  font-size: 12px;
  font-weight: 300;
  letter-spacing: 0.5px;
  color: var(--text-2);
}
.invalid-note.error {
  color: #cf4444;
}
/* 失效书源：整行置灰 + 名称/徽标红色 */
.source-row.invalid {
  opacity: 0.62;
}
.source-row.invalid .source-name {
  color: #cf4444;
}
.source-badge {
  flex-shrink: 0;
  padding: 1px 7px;
  border-radius: 4px;
  font-size: 10.5px;
  font-weight: 400;
  letter-spacing: 1px;
}
.source-badge.invalid {
  color: #cf4444;
  border: 1px solid rgba(207, 68, 68, 0.5);
  background: rgba(207, 68, 68, 0.06);
}

/* 测试按钮（细字描边） */
.test-btn {
  flex-shrink: 0;
  padding: 4px 12px;
  border-radius: var(--radius);
  border: 1px solid var(--border);
  background: none;
  color: var(--text-2);
  font-family: inherit;
  font-size: 11.5px;
  font-weight: 400;
  letter-spacing: 1px;
  cursor: pointer;
  transition:
    color 0.2s ease,
    border-color 0.2s ease,
    background-color 0.2s ease;
}
.test-btn:hover {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-soft);
}

/* ================= 书源调试弹窗 ================= */
.dlg-debug {
  width: min(480px, 100%);
}
.debug-actions {
  display: flex;
  gap: 6px;
  margin-bottom: 10px;
}
.debug-act {
  padding: 4px 16px;
}
.debug-input {
  width: 100%;
  height: 36px;
  margin-bottom: 10px;
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
.debug-input::placeholder {
  color: var(--text-3);
  font-weight: 300;
}
.debug-input:focus {
  border-color: var(--accent);
  background: var(--surface);
}
.debug-input:disabled {
  opacity: 0.55;
}
.debug-log {
  min-height: 96px;
  max-height: 200px;
  margin: 8px 0 0;
  padding: 10px 12px;
  overflow-y: auto;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--bg);
  font-family: 'SF Mono', 'JetBrains Mono', Consolas, monospace;
  font-size: 11.5px;
  line-height: 1.8;
}
.debug-line {
  margin: 0;
  color: var(--text-2);
  white-space: pre-wrap;
  word-break: break-all;
}
.debug-line.error {
  color: #cf4444;
}
.debug-line.running {
  color: var(--text-3);
}
.debug-msg {
  margin: 10px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-2);
}
.debug-msg.error {
  color: #cf4444;
}

/* ================= 订阅源区块 ================= */
.subs-section {
  margin-top: 40px;
  padding-top: 24px;
  border-top: 1px solid var(--border);
}
.subs-head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 14px;
}
.subs-title {
  margin: 0;
  font-size: 14px;
  font-weight: 400;
  letter-spacing: 2px;
  color: var(--text-1);
}
.subs-sub {
  font-size: 11.5px;
  font-weight: 300;
  color: var(--text-3);
}
.subs-add {
  display: flex;
  gap: 8px;
}
.subs-input {
  flex: 1;
  min-width: 0;
  padding: 0 12px;
}
.subs-msg {
  margin: 10px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-2);
}
.subs-msg.error {
  color: #cf4444;
}
.subs-empty {
  margin: 16px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
}
.subs-list {
  list-style: none;
  margin: 14px 0 0;
  padding: 0;
}
.subs-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 11px 6px;
  border-bottom: 1px solid var(--border);
}
.subs-row:first-child {
  border-top: 1px solid var(--border);
}
.subs-main {
  flex: 1;
  min-width: 0;
}
.subs-name {
  margin: 0;
  font-size: 13px;
  font-weight: 400;
  color: var(--text-1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.subs-url {
  margin: 3px 0 0;
  font-size: 12px;
  font-weight: 300;
  color: var(--text-3);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ================= 弹窗（极简，自写轻量） ================= */
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
  background: #f4f4f5;
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
  font-weight: 400;
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
.field-tip.error {
  color: #cf4444;
}
.confirm-text {
  margin: 0 0 18px;
  font-size: 13px;
  font-weight: 300;
  line-height: 1.7;
  color: var(--text-2);
}
.dlg-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 4px;
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
    padding: 12px 16px;
  }
  .content {
    padding: 32px 16px 56px;
  }
  .filter-row {
    flex-direction: column;
    align-items: stretch;
    gap: 12px;
  }
  .filter-box {
    width: 100%;
  }
  .source-group {
    display: none;
  }
  .source-state {
    display: none;
  }
  .dlg-overlay {
    padding: 16px;
  }
}
</style>
