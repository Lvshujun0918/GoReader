import { defineStore } from 'pinia'
import { ref } from 'vue'

const TOKEN_KEY = 'reader_access_token'
const USERNAME_KEY = 'reader_username'

export const useUserStore = defineStore('user', () => {
  const accessToken = ref(localStorage.getItem(TOKEN_KEY) || '')
  const username = ref(localStorage.getItem(USERNAME_KEY) || '')

  function setSession(token: string, name: string) {
    accessToken.value = token
    username.value = name
    localStorage.setItem(TOKEN_KEY, token)
    localStorage.setItem(USERNAME_KEY, name)
  }

  function clear() {
    accessToken.value = ''
    username.value = ''
    localStorage.removeItem(TOKEN_KEY)
    localStorage.removeItem(USERNAME_KEY)
  }

  return { accessToken, username, setSession, clear }
})
