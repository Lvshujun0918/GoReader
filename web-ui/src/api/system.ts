import request from './request'
import type { ReturnData, SystemInfo } from '@/types'

/**
 * 系统信息
 *
 * GET /reader3/getSystemInfo → ReturnData<SystemInfo>（版本/端口/用户数/书数）
 */

/** GET /reader3/getSystemInfo */
export function getSystemInfo(): Promise<ReturnData<SystemInfo>> {
  return request.get('/getSystemInfo').then((r) => r.data as ReturnData<SystemInfo>)
}
