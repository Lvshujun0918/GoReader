<script setup lang="ts">
import { onMounted } from 'vue'
import { applyUiTheme, loadUiTheme } from '@/utils/uiTheme'

onMounted(() => {
  // 界面主题（浅色/深色/跟随系统）：进入即恢复，并监听系统深色偏好（system 时自动切换）
  applyUiTheme(loadUiTheme())
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  const onSystemChange = () => {
    if (loadUiTheme() === 'system') applyUiTheme('system')
  }
  mq.addEventListener('change', onSystemChange)
})
</script>

<template>
  <router-view v-slot="{ Component }">
    <transition name="page" mode="out-in">
      <component :is="Component" />
    </transition>
  </router-view>
</template>
