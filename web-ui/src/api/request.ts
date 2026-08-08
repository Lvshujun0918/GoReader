import axios from 'axios'
import { ElMessage } from 'element-plus'
import router from '@/router'
import { useUserStore } from '@/stores/user'
import { notifyBackendReachable } from './backendFlag'
import type { ReturnData } from '@/types'

/** 自定义请求配置：silent=true 时失败不弹全局错误提示（探测待实现后端契约接口等场景，调用方自行降级处理） */
declare module 'axios' {
  export interface AxiosRequestConfig {
    silent?: boolean
  }
}

/** 请求选项 */
export interface RequestOptions {
  /** 静默模式：业务失败 / HTTP 错误均不弹 ElMessage（探测未实现契约接口用） */
  silent?: boolean
  /** 中止信号（axios 原生支持，用于搜索取消等场景） */
  signal?: AbortSignal
  /** 自定义超时（ms；缺省用实例默认 15s） */
  timeout?: number
}

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
    // 任一后端响应（含业务失败）都证明后端可达 → 复位降级模块的 backendDown 短路标志
    // （P2：backendDown 永不重置修复——网络恢复/重新登录后自动回到后端优先）
    notifyBackendReachable()
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
        const err = new Error(res.errorMsg || '请求失败') as Error & { data?: unknown }
        err.data = res.data
        // NEED_SECURE_KEY：不弹全局提示，由调用方（用户管理）引导输入 secureKey
        if (res.data !== 'NEED_SECURE_KEY') {
          const silent = !!(response.config as { silent?: boolean }).silent
          if (!silent) ElMessage.error(res.errorMsg || '请求失败')
        }
        return Promise.reject(err)
      }
      return response
    }
    return response
  },
  (error) => {
    // 有 HTTP 响应（4xx/5xx）说明后端可达；纯网络错误不算
    if (error.response) notifyBackendReachable()
    const silent = !!(error.config as { silent?: boolean } | undefined)?.silent
    if (error.response?.status === 401) {
      const store = useUserStore()
      store.clear()
      void router.replace({ path: '/login', query: { redirect: router.currentRoute.value.fullPath } })
    }
    if (!silent) ElMessage.error(error.response?.data?.errorMsg || error.message || '网络错误')
    return Promise.reject(error)
  },
)

export function get<T>(
  url: string,
  params?: Record<string, unknown>,
  opts?: RequestOptions,
): Promise<ReturnData<T>> {
  return request.get(url, { params, silent: opts?.silent }).then((r) => r.data as ReturnData<T>)
}

/** 第三参数兼容两种用法：历史调用传 query params；新调用传 RequestOptions（如 { silent: true }） */
export function post<T>(
  url: string,
  data?: unknown,
  paramsOrOpts?: Record<string, unknown> | RequestOptions,
): Promise<ReturnData<T>> {
  const isOpts = !!paramsOrOpts && ('silent' in paramsOrOpts || 'signal' in paramsOrOpts || 'timeout' in paramsOrOpts)
  const params = isOpts ? undefined : (paramsOrOpts as Record<string, unknown> | undefined)
  const opts = isOpts ? (paramsOrOpts as RequestOptions) : undefined
  return request
    .post(url, data, { params, silent: opts?.silent, signal: opts?.signal, timeout: opts?.timeout })
    .then((r) => r.data as ReturnData<T>)
}

export default request
