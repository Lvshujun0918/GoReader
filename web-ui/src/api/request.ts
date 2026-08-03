import axios from 'axios'
import { ElMessage } from 'element-plus'
import router from '@/router'
import { useUserStore } from '@/stores/user'
import type { ReturnData } from '@/types'

/** axios 实例：baseURL=/reader3，accessToken 自动携带（query），401/NEED_LOGIN 跳登录 */
const request = axios.create({
  baseURL: '/reader3',
  timeout: 15000,
})

request.interceptors.request.use((config) => {
  const store = useUserStore()
  if (store.accessToken) {
    config.params = { ...config.params, accessToken: store.accessToken }
  }
  return config
})

request.interceptors.response.use(
  (response) => {
    const res = response.data as ReturnData
    // 兼容 legacy：HTTP 恒为 200，业务结果在 isSuccess
    if (res && typeof res === 'object' && 'isSuccess' in res) {
      if (!res.isSuccess) {
        if (res.data === 'NEED_LOGIN' || (res.errorMsg || '').includes('请登录')) {
          const store = useUserStore()
          store.clear()
          void router.replace({ path: '/login', query: { redirect: router.currentRoute.value.fullPath } })
          return Promise.reject(new Error(res.errorMsg || '请登录后使用'))
        }
        ElMessage.error(res.errorMsg || '请求失败')
        return Promise.reject(new Error(res.errorMsg || '请求失败'))
      }
      return response
    }
    return response
  },
  (error) => {
    if (error.response?.status === 401) {
      const store = useUserStore()
      store.clear()
      void router.replace({ path: '/login', query: { redirect: router.currentRoute.value.fullPath } })
    }
    ElMessage.error(error.response?.data?.errorMsg || error.message || '网络错误')
    return Promise.reject(error)
  },
)

export function get<T>(url: string, params?: Record<string, unknown>): Promise<ReturnData<T>> {
  return request.get(url, { params }).then((r) => r.data as ReturnData<T>)
}

export function post<T>(url: string, data?: unknown): Promise<ReturnData<T>> {
  return request.post(url, data).then((r) => r.data as ReturnData<T>)
}

export default request
