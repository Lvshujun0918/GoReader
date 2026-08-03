<script setup lang="ts">
import { reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import LogoMark from '@/components/LogoMark.vue'
import { login as loginApi } from '@/api/auth'
import { useUserStore } from '@/stores/user'

const router = useRouter()
const route = useRoute()
const store = useUserStore()

const mode = ref<'login' | 'register'>('login')
const loading = ref(false)
const form = reactive({ username: '', password: '' })

function switchMode(m: 'login' | 'register') {
  mode.value = m
}

async function submit() {
  const { username, password } = form
  if (!username.trim() || !password) {
    ElMessage.warning('请输入用户名和密码')
    return
  }
  if (mode.value === 'register' && username.trim().length < 5) {
    ElMessage.warning('用户名不能低于 5 位（字母/数字）')
    return
  }
  if (password.length < 8) {
    ElMessage.warning('密码不能低于 8 位')
    return
  }

  loading.value = true
  try {
    const res = await loginApi({
      username: username.trim(),
      password,
      isLogin: mode.value === 'login',
    })
    store.setSession(res.data.accessToken, res.data.username)
    ElMessage.success(mode.value === 'login' ? '欢迎回来' : '注册成功，已自动登录')
    const redirect = typeof route.query.redirect === 'string' ? route.query.redirect : '/'
    await router.replace(redirect)
  } catch {
    // 错误提示已由 axios 拦截器统一处理
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="login-page">
    <!-- 背景光斑 -->
    <div class="login-bg" aria-hidden="true">
      <div class="orb orb-1"></div>
      <div class="orb orb-2"></div>
      <div class="orb orb-3"></div>
      <div class="orb orb-4"></div>
    </div>

    <main class="login-card glass fade-up">
      <div class="card-glow" aria-hidden="true"></div>

      <div class="login-logo">
        <LogoMark />
        <h1 class="login-title">夜读</h1>
        <p class="login-sub">Reader · 沉浸式阅读</p>
      </div>

      <!-- 登录 / 注册 切换 -->
      <div class="mode-switch" role="tablist">
        <button
          type="button"
          :class="{ active: mode === 'login' }"
          @click="switchMode('login')"
        >
          登录
        </button>
        <button
          type="button"
          :class="{ active: mode === 'register' }"
          @click="switchMode('register')"
        >
          注册
        </button>
      </div>

      <form class="login-form" @submit.prevent="submit">
        <label class="field">
          <span class="field-label">用户名</span>
          <el-input
            v-model="form.username"
            placeholder="请输入用户名"
            maxlength="32"
            autocomplete="username"
            size="large"
          >
            <template #prefix>
              <svg class="input-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                <circle cx="12" cy="8" r="3.6" />
                <path d="M4.5 20c1.4-3.4 4.3-5 7.5-5s6.1 1.6 7.5 5" />
              </svg>
            </template>
          </el-input>
        </label>

        <label class="field">
          <span class="field-label">密码</span>
          <el-input
            v-model="form.password"
            type="password"
            placeholder="请输入密码（至少 8 位）"
            show-password
            maxlength="64"
            autocomplete="current-password"
            size="large"
            @keyup.enter="submit"
          >
            <template #prefix>
              <svg class="input-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
                <rect x="5" y="10.5" width="14" height="9.5" rx="2.6" />
                <path d="M8 10.5V8a4 4 0 0 1 8 0v2.5" />
              </svg>
            </template>
          </el-input>
        </label>

        <button class="submit-btn" type="submit" :disabled="loading">
          <span v-if="loading" class="btn-spinner" aria-hidden="true"></span>
          <span v-else>{{ mode === 'login' ? '登 录' : '注 册' }}</span>
        </button>
      </form>

      <p class="login-foot">
        {{ mode === 'login' ? '还没有账号？' : '已有账号？' }}
        <button type="button" class="link-btn" @click="switchMode(mode === 'login' ? 'register' : 'login')">
          {{ mode === 'login' ? '立即注册' : '去登录' }}
        </button>
      </p>
    </main>

    <footer class="login-footer">reader-dev · Rust 后端 /reader3 API</footer>
  </div>
</template>

<style scoped>
.login-page {
  position: relative;
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  padding: 24px;
}

/* ---------- 背景光斑 ---------- */
.login-bg {
  position: fixed;
  inset: 0;
  pointer-events: none;
}
.orb {
  position: absolute;
  border-radius: 50%;
  filter: blur(90px);
  opacity: 0.55;
  animation: orb-float 18s ease-in-out infinite alternate;
}
.orb-1 {
  width: 440px;
  height: 440px;
  top: -140px;
  left: -100px;
  background: radial-gradient(circle, rgba(99, 102, 241, 0.6), transparent 65%);
}
.orb-2 {
  width: 400px;
  height: 400px;
  bottom: -120px;
  right: -80px;
  background: radial-gradient(circle, rgba(34, 211, 238, 0.35), transparent 65%);
  animation-delay: -6s;
}
.orb-3 {
  width: 320px;
  height: 320px;
  top: 42%;
  left: 56%;
  background: radial-gradient(circle, rgba(168, 85, 247, 0.42), transparent 65%);
  animation-delay: -11s;
}
.orb-4 {
  width: 240px;
  height: 240px;
  top: 12%;
  right: 22%;
  background: radial-gradient(circle, rgba(56, 189, 248, 0.3), transparent 65%);
  animation-delay: -3s;
}
@keyframes orb-float {
  from {
    transform: translate(0, 0) scale(1);
  }
  to {
    transform: translate(46px, 34px) scale(1.14);
  }
}

/* ---------- 登录卡片 ---------- */
.login-card {
  position: relative;
  width: min(420px, 100%);
  padding: 44px 40px 32px;
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-card);
  overflow: hidden;
  animation-delay: 0.05s;
}
.card-glow {
  position: absolute;
  top: -90px;
  left: 50%;
  transform: translateX(-50%);
  width: 340px;
  height: 180px;
  background: var(--grad-brand);
  filter: blur(70px);
  opacity: 0.22;
  pointer-events: none;
}

.login-logo {
  display: flex;
  flex-direction: column;
  align-items: center;
  margin-bottom: 26px;
}
.login-title {
  margin: 14px 0 0;
  font-size: 28px;
  font-weight: 700;
  letter-spacing: 8px;
  background: linear-gradient(120deg, #e0e7ff 0%, #a5b4fc 50%, #67e8f9 100%);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}
.login-sub {
  margin: 6px 0 0;
  font-size: 12.5px;
  letter-spacing: 2px;
  color: var(--text-3);
}

/* ---------- 模式切换 ---------- */
.mode-switch {
  display: flex;
  gap: 4px;
  padding: 4px;
  margin-bottom: 26px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--glass-border);
}
.mode-switch button {
  flex: 1;
  padding: 8px 0;
  border: none;
  border-radius: 999px;
  background: transparent;
  color: var(--text-2);
  font-size: 13.5px;
  cursor: pointer;
  transition: all 0.3s var(--ease-out);
}
.mode-switch button:hover {
  color: var(--text-1);
}
.mode-switch button.active {
  background: linear-gradient(135deg, var(--brand-1), var(--brand-2));
  color: #fff;
  font-weight: 600;
  box-shadow: 0 6px 16px -6px rgba(99, 102, 241, 0.7);
}

/* ---------- 表单 ---------- */
.login-form {
  display: flex;
  flex-direction: column;
  gap: 18px;
}
.field {
  display: block;
}
.field-label {
  display: block;
  margin-bottom: 8px;
  font-size: 12.5px;
  color: var(--text-2);
  letter-spacing: 1px;
}

/* Element Plus 输入框深色玻璃化 */
.login-form :deep(.el-input) {
  --el-input-height: 46px;
  --el-input-bg-color: transparent;
  --el-input-border-color: transparent;
  --el-input-hover-border-color: transparent;
  --el-input-focus-border-color: transparent;
  --el-input-text-color: var(--text-1);
  --el-input-placeholder-color: var(--text-3);
}
.login-form :deep(.el-input__wrapper) {
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  box-shadow: none !important;
  padding: 2px 14px;
  transition: border-color 0.3s, box-shadow 0.3s, background 0.3s;
}
.login-form :deep(.el-input__wrapper:hover) {
  border-color: rgba(255, 255, 255, 0.2);
}
.login-form :deep(.el-input__wrapper.is-focus) {
  border-color: var(--brand-1);
  background: rgba(255, 255, 255, 0.07);
  box-shadow: 0 0 0 3.5px rgba(99, 102, 241, 0.16) !important;
}
.login-form :deep(.el-input__prefix) {
  color: var(--text-3);
}
.input-icon {
  width: 16px;
  height: 16px;
  display: block;
}
.login-form :deep(.el-input__inner) {
  font-size: 14.5px;
}
.login-form :deep(.el-input__password) {
  color: var(--text-3);
}

/* ---------- 提交按钮 ---------- */
.submit-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  width: 100%;
  height: 48px;
  margin-top: 6px;
  border: none;
  border-radius: 12px;
  background: var(--grad-brand);
  background-size: 160% 160%;
  color: #fff;
  font-size: 15px;
  font-weight: 600;
  letter-spacing: 6px;
  cursor: pointer;
  box-shadow: 0 12px 26px -12px rgba(99, 102, 241, 0.65);
  transition:
    transform 0.25s var(--ease-out),
    box-shadow 0.3s,
    filter 0.3s,
    background-position 0.6s;
}
.submit-btn:hover:not(:disabled) {
  transform: translateY(-2px);
  filter: brightness(1.1);
  box-shadow: 0 16px 32px -12px rgba(99, 102, 241, 0.8);
  background-position: 100% 100%;
}
.submit-btn:active:not(:disabled) {
  transform: translateY(0) scale(0.985);
}
.submit-btn:disabled {
  opacity: 0.72;
  cursor: not-allowed;
}
.btn-spinner {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 2.5px solid rgba(255, 255, 255, 0.35);
  border-top-color: #fff;
  animation: spin 0.7s linear infinite;
}

/* ---------- 底部 ---------- */
.login-foot {
  margin: 22px 0 0;
  text-align: center;
  font-size: 13px;
  color: var(--text-3);
}
.link-btn {
  border: none;
  background: none;
  padding: 0;
  color: #a5b4fc;
  font-size: 13px;
  cursor: pointer;
  transition: color 0.25s;
}
.link-btn:hover {
  color: #67e8f9;
}

.login-footer {
  position: fixed;
  bottom: 18px;
  left: 0;
  right: 0;
  text-align: center;
  font-size: 11.5px;
  letter-spacing: 1px;
  color: var(--text-3);
}
</style>
