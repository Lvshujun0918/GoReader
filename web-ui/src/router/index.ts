import { createRouter, createWebHistory } from 'vue-router'
import { useUserStore } from '@/stores/user'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/login',
      name: 'login',
      component: () => import('@/views/LoginView.vue'),
      meta: { title: '登录' },
    },
    {
      path: '/',
      name: 'bookshelf',
      component: () => import('@/views/BookshelfView.vue'),
      meta: { title: '书架' },
    },
    {
      path: '/book/:url',
      name: 'book-detail',
      component: () => import('@/views/BookDetailView.vue'),
      meta: { title: '书籍详情' },
    },
    {
      path: '/reader/:bookUrl',
      name: 'reader',
      component: () => import('@/views/ReaderView.vue'),
      meta: { title: '阅读' },
    },
    {
      path: '/search',
      name: 'search',
      component: () => import('@/views/SearchView.vue'),
      meta: { title: '搜索' },
    },
    {
      path: '/explore',
      name: 'explore',
      component: () => import('@/views/ExploreView.vue'),
      meta: { title: '探索' },
    },
    {
      path: '/sources',
      name: 'sources',
      component: () => import('@/views/SourceManageView.vue'),
      meta: { title: '书源管理' },
    },
    {
      path: '/rules',
      name: 'rules',
      component: () => import('@/views/ReplaceRuleView.vue'),
      meta: { title: '替换规则' },
    },
    {
      path: '/rss',
      name: 'rss',
      component: () => import('@/views/RssView.vue'),
      meta: { title: 'RSS' },
    },
    {
      path: '/settings',
      name: 'settings',
      component: () => import('@/views/SettingsView.vue'),
      meta: { title: '设置' },
    },
    {
      path: '/files',
      name: 'files',
      component: () => import('@/views/FileManageView.vue'),
      meta: { title: '文件' },
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

router.beforeEach((to) => {
  const store = useUserStore()
  if (to.path !== '/login' && !store.accessToken) {
    return { path: '/login', query: { redirect: to.fullPath } }
  }
  if (to.path === '/login' && store.accessToken) {
    return { path: '/' }
  }
  return true
})

router.afterEach((to) => {
  document.title = `${String(to.meta.title ?? '')} · 夜读`
})

export default router
