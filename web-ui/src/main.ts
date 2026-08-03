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

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.use(ElementPlus)
app.directive('lazy', lazy)
app.mount('#app')
