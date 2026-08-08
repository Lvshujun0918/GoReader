<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { getServerStats } from '@/api/serverStats'
import TopNav from '@/components/TopNav.vue'
import type { ServerStats } from '@/types'

/**
 * 服务监控（GET /reader3/getServerStats）
 * 卡片：内存 / CPU / 请求量（总数·今日·Top 接口）/ 在线会话 + 运行信息
 * 纯 CSS 条形（无图表库）；10s 自动刷新（页面隐藏时暂停，恢复可见立即刷新）。
 */

const REFRESH_MS = 10_000

const stats = ref<ServerStats | null>(null)
const error = ref('')
const loading = ref(false)
const lastUpdated = ref(0)
/** 距下次自动刷新的秒数（倒计时显示） */
const countdown = ref(REFRESH_MS / 1000)
let timer: number | undefined

async function refresh() {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const res = await getServerStats()
    stats.value = res.data ?? null
    lastUpdated.value = Date.now()
  } catch (err) {
    error.value = (err as { message?: string } | null | undefined)?.message ?? '监控数据加载失败'
  } finally {
    loading.value = false
    countdown.value = REFRESH_MS / 1000
  }
}

function onTick() {
  if (document.hidden) return // 后台标签页不刷（恢复可见时立即刷新）
  countdown.value -= 1
  if (countdown.value <= 0) {
    void refresh()
  }
}

function onVisible() {
  if (!document.hidden) void refresh()
}

onMounted(() => {
  void refresh()
  timer = window.setInterval(onTick, 1000)
  document.addEventListener('visibilitychange', onVisible)
})

onBeforeUnmount(() => {
  if (timer !== undefined) window.clearInterval(timer)
  document.removeEventListener('visibilitychange', onVisible)
})

/* ---------------- 格式化 ---------------- */

function fmtMb(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`
}

function fmtTime(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

function fmtUptime(sec: number): string {
  const d = Math.floor(sec / 86_400)
  const h = Math.floor((sec % 86_400) / 3600)
  const m = Math.floor((sec % 3600) / 60)
  if (d > 0) return `${d}${'天'} ${h}${'时'}`
  if (h > 0) return `${h}${'时'} ${m}${'分'}`
  return `${Math.max(m, 1)}${'分'}`
}

/** 百分比条宽度（0..100 夹取） */
function barWidth(pct: number): string {
  return `${Math.min(100, Math.max(0, pct))}%`
}

const memPercent = computed(() => stats.value?.memory.percent ?? 0)
const cpuPercent = computed(() => stats.value?.cpu.percent ?? 0)
const topEndpoints = computed(() => stats.value?.requests?.topEndpoints?.slice(0, 5) ?? [])

/* ---------------- 运行信息 ---------------- */
</script>

<template>
  <div class="monitor-page">
    <!-- 统一顶栏菜单（P3-A：TopNav nav + 二级菜单） -->
    <TopNav active="/server-stats" show-users-link />

    <main class="content">
      <div class="section-head">
        <h1 class="section-title">{{ '服务监控' }}</h1>
        <span class="count">{{ countdown }}s 后自动刷新</span>
        <button class="refresh-btn" type="button" :disabled="loading" @click="refresh">
          {{ loading ? '加载中…' : '刷新' }}
        </button>
      </div>

      <div v-if="error" class="err-note">{{ error }}</div>

      <!-- 骨架/空态 -->
      <div v-if="!stats && !error" class="cards">
        <div v-for="i in 4" :key="i" class="card skeleton">
          <div class="skeleton-line w60"></div>
          <div class="skeleton-line w90"></div>
        </div>
      </div>

      <div v-else-if="stats" class="cards">
        <!-- 内存 -->
        <div class="card">
          <div class="card-title">{{ '内存' }}</div>
          <div class="big-num">{{ fmtMb(stats?.memory?.usedMb ?? 0) }} <span class="sub">/ {{ fmtMb(stats?.memory?.totalMb ?? 0) }}</span></div>
          <div class="bar">
            <div class="bar-fill mem" :style="{ width: barWidth(memPercent) }"></div>
          </div>
          <div class="bar-label">
            <span>已用 {{ memPercent.toFixed(1) }}%</span>
            <span>{{ '可用' }} {{ fmtMb(stats?.memory?.availableMb ?? 0) }}</span>
          </div>
          <div class="card-sub">{{ '本进程内存' }} {{ fmtMb(stats?.memory?.processMb ?? 0) }}</div>
        </div>

        <!-- CPU -->
        <div class="card">
          <div class="card-title">{{ 'CPU' }}</div>
          <div class="big-num">{{ cpuPercent.toFixed(1) }}<span class="sub">%</span></div>
          <div class="bar">
            <div class="bar-fill cpu" :style="{ width: barWidth(cpuPercent) }"></div>
          </div>
          <div class="bar-label">
            <span>{{ '使用率' }}</span>
            <span>{{ stats?.cpu?.cores ?? 0 }} {{ '核心' }}</span>
          </div>
          <div class="card-sub">{{ '短采样（约 200ms）' }}</div>
        </div>

        <!-- 请求量 -->
        <div class="card">
          <div class="card-title">{{ '请求量' }}</div>
          <div class="big-num">{{ (stats?.requests?.total ?? 0).toLocaleString() }} <span class="sub">{{ '累计' }}</span></div>
          <div class="today-line">{{ '今日' }}：{{ (stats?.requests?.today ?? 0).toLocaleString() }}</div>
          <div v-if="topEndpoints.length" class="endpoint-list">
            <div v-for="(ep, i) in topEndpoints" :key="ep.path" class="endpoint-row">
              <span class="ep-rank">{{ i + 1 }}</span>
              <span class="ep-path" :title="ep.path">{{ ep.path }}</span>
              <span class="ep-count">{{ ep.count.toLocaleString() }}</span>
            </div>
          </div>
          <div v-else class="card-sub">{{ '暂无请求' }}</div>
        </div>

        <!-- 在线会话 -->
        <div class="card">
          <div class="card-title">{{ '在线会话' }}</div>
          <div class="big-num">{{ stats?.online?.sessions ?? 0 }} <span class="sub">{{ '个' }}</span></div>
          <div class="card-sub">{{ '有效 token 数' }}</div>
        </div>

        <!-- 运行信息 -->
        <div class="card">
          <div class="card-title">{{ '运行信息' }}</div>
          <div class="kv-row"><span>{{ '版本' }}</span><span>v{{ stats.version }}</span></div>
          <div class="kv-row"><span>{{ '端口' }}</span><span>{{ stats.port }}</span></div>
          <div class="kv-row"><span>{{ '运行时长' }}</span><span>{{ fmtUptime(stats.uptimeSeconds) }}</span></div>
          <div class="kv-row"><span>{{ '最近刷新' }}</span><span>{{ lastUpdated ? fmtTime(lastUpdated) : '—' }}</span></div>
        </div>
      </div>
    </main>
  </div>
</template>

<style scoped>
.monitor-page {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  animation: fade-in 0.2s ease both;
}

/* ================= 顶部导航（与书仓页一致） ================= */
.topbar {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  gap: 24px;
  padding: 14px 32px;
  background: var(--bg-float);
  border-bottom: 1px solid var(--border);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  font-weight: 700;
  font-size: 17px;
}
.brand-logo {
  width: 26px;
  height: 26px;
  border-radius: 7px;
}
.brand-name {
  color: var(--text-1);
}
.brand-dot {
  color: var(--accent);
}
.user-area {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 6px;
}
.nav-link {
  border: none;
  background: none;
  color: var(--text-2);
  font-size: 14px;
  padding: 6px 10px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.nav-link:hover {
  background: var(--bg-hover);
  color: var(--text-1);
}
.nav-link.active {
  color: var(--accent);
  font-weight: 600;
}
.user-chip {
  margin-left: 8px;
  font-size: 13px;
  color: var(--text-3);
  max-width: 140px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ================= 内容 ================= */
.content {
  flex: 1;
  width: 100%;
  max-width: 1080px;
  margin: 0 auto;
  padding: 20px 24px 48px;
}
.section-head {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 18px;
}
.section-title {
  font-size: 22px;
  color: var(--text-1);
  margin: 0;
}
.count {
  font-size: 13px;
  color: var(--text-3);
}
.refresh-btn {
  margin-left: auto;
  border: 1px solid var(--border);
  background: var(--bg-float);
  color: var(--text-1);
  font-size: 13px;
  padding: 6px 14px;
  border-radius: 8px;
  cursor: pointer;
}
.refresh-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.err-note {
  margin-bottom: 14px;
  padding: 10px 14px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--danger, var(--danger)) 12%, transparent);
  color: var(--danger, var(--danger));
  font-size: 13px;
}

/* ================= 卡片 ================= */
.cards {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 16px;
}
.card {
  background: var(--bg-float);
  border: 1px solid var(--border);
  border-radius: 14px;
  padding: 16px 18px;
  min-height: 150px;
}
.card-title {
  font-size: 13px;
  color: var(--text-3);
  margin-bottom: 10px;
}
.big-num {
  font-size: 30px;
  font-weight: 700;
  color: var(--text-1);
  line-height: 1.2;
  margin-bottom: 10px;
  font-variant-numeric: tabular-nums;
}
.big-num .sub {
  font-size: 14px;
  font-weight: 400;
  color: var(--text-3);
}
.big-num.good {
  color: var(--ok, var(--success));
}
.big-num.mid {
  color: var(--warn, #f5a524);
}
.big-num.bad {
  color: var(--danger, var(--danger));
}

/* ================= 纯 CSS 条形 ================= */
.bar {
  height: 10px;
  border-radius: 5px;
  background: var(--bg-hover);
  overflow: hidden;
  margin-bottom: 8px;
}
.bar-fill {
  height: 100%;
  border-radius: 5px;
  transition: width 0.6s ease;
}
.bar-fill.mem {
  background: linear-gradient(90deg, var(--success), #46a758);
}
.bar-fill.cpu {
  background: linear-gradient(90deg, #3e63dd, #5e8af0);
}
.bar-fill.good {
  background: linear-gradient(90deg, var(--success), #46a758);
}
.bar-fill.mid {
  background: linear-gradient(90deg, #f5a524, #f7b750);
}
.bar-fill.bad {
  background: linear-gradient(90deg, var(--danger), #f2555a);
}
.bar-label {
  display: flex;
  justify-content: space-between;
  font-size: 12px;
  color: var(--text-3);
  margin-bottom: 6px;
}
.card-sub {
  font-size: 12px;
  color: var(--text-3);
  margin-top: 8px;
}
.today-line {
  font-size: 13px;
  color: var(--text-2);
  margin-bottom: 8px;
}

/* ================= 请求 Top 列表 ================= */
.endpoint-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.endpoint-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--text-2);
}
.ep-rank {
  width: 16px;
  text-align: center;
  color: var(--text-3);
  font-variant-numeric: tabular-nums;
}
.ep-path {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: ui-monospace, monospace;
}
.ep-count {
  color: var(--text-1);
  font-variant-numeric: tabular-nums;
}

/* ================= 运行信息 ================= */
.kv-row {
  display: flex;
  justify-content: space-between;
  font-size: 13px;
  color: var(--text-2);
  padding: 5px 0;
}
.kv-row span:last-child {
  color: var(--text-1);
  font-variant-numeric: tabular-nums;
}
.empty-note {
  font-size: 13px;
  color: var(--text-3);
  padding: 12px 0;
}

/* ================= 骨架 ================= */
.skeleton {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.skeleton-line {
  height: 14px;
  border-radius: 7px;
  background: var(--bg-hover);
  animation: pulse 1.2s ease-in-out infinite;
}
.skeleton-line.w60 {
  width: 60%;
}
.skeleton-line.w90 {
  width: 90%;
}
@keyframes pulse {
  0%,
  100% {
    opacity: 0.5;
  }
  50% {
    opacity: 1;
  }
}
</style>
