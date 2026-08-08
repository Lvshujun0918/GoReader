/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ['class'],
  content: ['./index.html', './src/**/*.{vue,js,ts,jsx,tsx}'],
  theme: {
    container: {
      center: true,
      padding: '2rem',
      screens: {
        '2xl': '1400px',
      },
    },
    extend: {
      colors: {
        border: 'hsl(var(--ui-border))',
        input: 'hsl(var(--ui-input))',
        ring: 'hsl(var(--ui-ring))',
        background: 'hsl(var(--ui-background))',
        foreground: 'hsl(var(--ui-foreground))',
        primary: {
          DEFAULT: 'hsl(var(--ui-primary))',
          foreground: 'hsl(var(--ui-primary-foreground))',
        },
        secondary: {
          DEFAULT: 'hsl(var(--ui-secondary))',
          foreground: 'hsl(var(--ui-secondary-foreground))',
        },
        destructive: {
          DEFAULT: 'hsl(var(--ui-destructive))',
          foreground: 'hsl(var(--ui-destructive-foreground))',
        },
        muted: {
          DEFAULT: 'hsl(var(--ui-muted))',
          foreground: 'hsl(var(--ui-muted-foreground))',
        },
        accent: {
          DEFAULT: 'hsl(var(--ui-accent))',
          foreground: 'hsl(var(--ui-accent-foreground))',
        },
        popover: {
          DEFAULT: 'hsl(var(--ui-popover))',
          foreground: 'hsl(var(--ui-popover-foreground))',
        },
        card: {
          DEFAULT: 'hsl(var(--ui-card))',
          foreground: 'hsl(var(--ui-card-foreground))',
        },
      },
      borderRadius: {
        lg: 'var(--ui-radius)',
        md: 'calc(var(--ui-radius) - 2px)',
        sm: 'calc(var(--ui-radius) - 4px)',
      },
      fontFamily: {
        sans: ['Inter', 'LXGW WenKai', 'system-ui', 'sans-serif'],
      },
      keyframes: {
        'accordion-down': {
          from: { height: '0' },
          to: { height: 'var(--reka-accordion-content-height)' },
        },
        'accordion-up': {
          from: { height: 'var(--reka-accordion-content-height)' },
          to: { height: '0' },
        },
      },
      animation: {
        'accordion-down': 'accordion-down 0.2s ease-out',
        'accordion-up': 'accordion-up 0.2s ease-out',
      },
    },
  },
  plugins: [require('tailwindcss-animate')],
}
