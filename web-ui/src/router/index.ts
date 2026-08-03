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
