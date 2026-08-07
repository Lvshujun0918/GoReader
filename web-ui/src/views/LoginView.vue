<script setup lang="ts">
import { reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessage } from 'element-plus'
import { login as loginApi } from '@/api/auth'
import { useUserStore } from '@/stores/user'
import { t } from '@/utils/i18n'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'

const router = useRouter()
const route = useRoute()
const store = useUserStore()

const mode = ref<'login' | 'register'>('login')
const loading = ref(false)
const form = reactive({ username: '', password: '', code: '' })
/** GAP 150：记住我——不勾选时 token 存 sessionStorage（关闭标签页即登出） */
const remember = ref(localStorage.getItem('reader_remember') !== '0')

function switchMode(m: 'login' | 'register') {
  mode.value = m
}

async function submit() {
  const { username, password } = form
  if (!username.trim() || !password) {
    ElMessage.warning(t('login.needBoth'))
    return
  }
  if (mode.value === 'register' && username.trim().length < 5) {
    ElMessage.warning(t('login.usernameTooShort'))
    return
  }
  if (mode.value === 'register' && password.length < 8) {
    ElMessage.warning(t('login.passwordTooShort'))
    return
  }

  loading.value = true
  try {
    const res = await loginApi({
      username: username.trim(),
      password,
      isLogin: mode.value === 'login',
      // GAP 90：注册模式携带邀请码（后端 register 校验 code 参数）
      code: mode.value === 'register' && form.code.trim() ? form.code.trim() : undefined,
    })
    store.setSession(res.data.accessToken, res.data.username, remember.value)
    ElMessage.success(mode.value === 'login' ? t('login.welcomeBack') : t('login.registered'))
    // GAP 127：登录成功回跳 redirect query（仅限站内路径，防开放重定向）
    const q = route.query.redirect
    const redirect =
      typeof q === 'string' && q.startsWith('/') && !q.startsWith('//') ? q : '/'
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
    <main class="login-panel">
      <!-- 徽标 + 字标 -->
      <div class="wordmark">
        <img class="login-logo" src="/logo.svg" :alt="t('brand.name')" />
        <h1 class="wordmark-text">{{ t('brand.name') }}<span class="wordmark-dot">.</span></h1>
        <p class="wordmark-sub">READER</p>
      </div>

      <!-- 登录 / 注册 切换（shadcn-vue Tabs） -->
      <Tabs :model-value="mode" class="login-tabs">
        <TabsList class="login-tabs-list">
          <TabsTrigger value="login" @click="switchMode('login')">
            {{ t('login.title') }}
          </TabsTrigger>
          <TabsTrigger value="register" @click="switchMode('register')">
            {{ t('login.register') }}
          </TabsTrigger>
        </TabsList>
      </Tabs>

      <!-- shadcn-vue Card 表单 -->
      <Card class="login-card">
        <CardContent class="pt-6">
          <form class="grid gap-4" @submit.prevent="submit">
            <div class="grid gap-2">
              <Label for="login-username">{{ t('login.username') }}</Label>
              <Input
                id="login-username"
                v-model="form.username"
                type="text"
                :placeholder="t('login.placeholder.username')"
                maxlength="32"
                autocomplete="username"
                spellcheck="false"
              />
            </div>

            <div class="grid gap-2">
              <Label for="login-password">{{ t('login.password') }}</Label>
              <Input
                id="login-password"
                v-model="form.password"
                type="password"
                :placeholder="t('login.placeholder.password')"
                maxlength="64"
                autocomplete="current-password"
              />
            </div>

            <!-- GAP 90：注册模式邀请码（后端开启邀请注册时必填；未开启可留空） -->
            <div v-if="mode === 'register'" class="grid gap-2">
              <Label for="login-code">{{ t('login.inviteCode') }}</Label>
              <Input
                id="login-code"
                v-model="form.code"
                type="text"
                :placeholder="t('login.placeholder.code')"
                maxlength="64"
                autocomplete="off"
                spellcheck="false"
              />
            </div>

            <!-- GAP 150：记住我（不勾选 → sessionStorage 存 token，关闭标签页即登出） -->
            <div class="flex items-center gap-2">
              <Checkbox id="login-remember" v-model:checked="remember" />
              <Label for="login-remember" class="font-normal cursor-pointer">
                {{ t('login.remember') }}
              </Label>
            </div>

            <Button type="submit" :disabled="loading" class="w-full">
              {{ mode === 'login' ? t('login.submit') : t('login.registerSubmit') }}
            </Button>
          </form>
        </CardContent>
      </Card>

      <p class="login-foot">
        {{ mode === 'login' ? t('login.noAccount') : t('login.hasAccount') }}
        <button
          type="button"
          class="link-btn"
          @click="switchMode(mode === 'login' ? 'register' : 'login')"
        >
          {{ mode === 'login' ? t('login.goRegister') : t('login.goLogin') }}
        </button>
      </p>
    </main>

    <footer class="login-footer">reader-dev · /reader3</footer>
  </div>
</template>

<style scoped>
.login-page {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  animation: fade-in 0.2s ease both;
}

.login-panel {
  width: min(360px, 100%);
  padding: 56px 8px 40px;
}

/* ---------- 字标 ---------- */
.wordmark {
  display: flex;
  flex-direction: column;
  align-items: center;
  margin-bottom: 36px;
}
.wordmark .login-logo {
  width: 72px;
  height: 72px;
  margin-bottom: 12px;
}
.wordmark-text {
  margin: 0;
  font-size: 34px;
  font-weight: 300;
  letter-spacing: 10px;
  text-indent: 10px; /* 抵消末字后的字距，视觉居中 */
  color: var(--text-1);
}
.wordmark-dot {
  color: var(--accent);
  font-weight: 400;
}
.wordmark-sub {
  margin: 10px 0 0;
  font-size: 11px;
  font-weight: 400;
  letter-spacing: 6px;
  text-indent: 6px;
  color: var(--text-3);
}

/* ---------- shadcn-vue 卡片 ---------- */
.login-tabs {
  display: flex;
  justify-content: center;
  margin-bottom: 24px;
}
.login-card {
  box-shadow: var(--shadow-1);
}
.login-foot {
  margin-top: 20px;
  text-align: center;
  font-size: 13px;
  color: var(--text-2);
}
.link-btn {
  color: var(--accent);
  background: none;
  border: none;
  padding: 0 4px;
  cursor: pointer;
  font-size: 13px;
}
.tg-foot {
  margin-left: 12px;
  color: var(--text-3);
  font-size: 12px;
  text-decoration: none;
}
.login-footer {
  position: fixed;
  bottom: 18px;
  left: 0;
  right: 0;
  text-align: center;
  font-size: 11px;
  letter-spacing: 1px;
  color: var(--text-3);
}
</style>
