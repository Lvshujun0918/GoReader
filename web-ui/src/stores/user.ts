import { defineStore } from 'pinia'
import { ref } from 'vue'

const TOKEN_KEY = 'reader_access_token'
const USERNAME_KEY = 'reader_username'
const REMEMBER_KEY = 'reader_remember'

/** 读取会话：勾选「记住我」时 token 在 localStorage（跨会话），否则在 sessionStorage（关标签页即登出） */
function readSession(): { token: string; username: string } {
  const token = localStorage.getItem(TOKEN_KEY) || sessionStorage.getItem(TOKEN_KEY) || ''
  const username = localStorage.getItem(USERNAME_KEY) || sessionStorage.getItem(USERNAME_KEY) || ''
  return { token, username }
}

export const useUserStore = defineStore('user', () => {
  const init = readSession()
  const accessToken = ref(init.token)
  const username = ref(init.username)

  /** GAP 150：remember=false 时 token 只写 sessionStorage（关闭标签页即登出） */
  function setSession(token: string, name: string, remember = true) {
    accessToken.value = token
    username.value = name
    localStorage.removeItem(TOKEN_KEY)
    localStorage.removeItem(USERNAME_KEY)
    sessionStorage.removeItem(TOKEN_KEY)
    sessionStorage.removeItem(USERNAME_KEY)
    const store = remember ? localStorage : sessionStorage
    try {
      store.setItem(TOKEN_KEY, token)
      store.setItem(USERNAME_KEY, name)
      localStorage.setItem(REMEMBER_KEY, remember ? '1' : '0')
    } catch {
      /* 存储不可用时仅内存会话 */
    }
  }

  function clear() {
    accessToken.value = ''
    username.value = ''
    localStorage.removeItem(TOKEN_KEY)
    localStorage.removeItem(USERNAME_KEY)
    sessionStorage.removeItem(TOKEN_KEY)
    sessionStorage.removeItem(USERNAME_KEY)
  }

  return { accessToken, username, setSession, clear }
})
