<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { useUserStore } from '@/stores/user'

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
