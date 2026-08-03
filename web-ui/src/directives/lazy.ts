import type { Directive } from 'vue'

const observers = new WeakMap<HTMLImageElement, IntersectionObserver>()

/** v-lazy：封面图懒加载（IntersectionObserver，进入视口附近才加载 + 淡入） */
export const lazy: Directive<HTMLImageElement, string> = {
  mounted(el, binding) {
    const src = binding.value
    if (!src) return
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            el.src = src
            el.classList.add('is-loaded')
            observer.disconnect()
            observers.delete(el)
          }
        }
      },
      { rootMargin: '240px 0px' },
    )
    observers.set(el, observer)
    observer.observe(el)
  },
  updated(el, binding) {
    if (binding.value && binding.value !== binding.oldValue) {
      el.src = binding.value
    }
  },
  unmounted(el) {
    observers.get(el)?.disconnect()
    observers.delete(el)
  },
}
