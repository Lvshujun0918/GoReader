import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ElementPlus from 'element-plus'
import 'element-plus/dist/index.css'
import 'element-plus/theme-chalk/dark/css-vars.css'

import App from './App.vue'
import router from './router'
import { lazy } from './directives/lazy'
import './styles/main.css'

// 主题由阅读页顶部按钮切换（html[data-theme=light|dark|paper]，见 styles/main.css）
// 旧 html.dark hack（强制 dark class + main.css 反向重映射）已清理

// GAP 75：内置网络字体加载失败提示（@font-face 已有 font-display: swap 兜底——
// 失败时浏览器回退系统字体，此处仅 console.warn 标注，便于排查）
if (typeof document !== 'undefined' && 'fonts' in document) {
  const FONT_FAMILIES = ['LXGW WenKai', 'Source Han Serif CN']
  for (const family of FONT_FAMILIES) {
    document.fonts
      .load(`16px "${family}"`, '永州之野产异蛇，黑质而白章')
      .then(
        (loaded) => {
          if (!loaded || loaded.length === 0) {
            console.warn(`[fonts] "${family}" 加载失败（font-display: swap 已回退系统字体）`)
          }
        },
        () => {
          console.warn(`[fonts] "${family}" 加载失败（font-display: swap 已回退系统字体）`)
        },
      )
  }
}

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.use(ElementPlus)
app.directive('lazy', lazy)
app.mount('#app')
