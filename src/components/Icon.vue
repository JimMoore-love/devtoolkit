<script setup>
defineProps({ name: { type: String, required: true }, size: { type: [Number, String], default: 18 } })

const paths = {
  dashboard: 'M3 3h8v8H3zM13 3h8v5h-8zM13 10h8v11h-8zM3 13h8v8H3z',
  ports: 'M5 16a6 6 0 0 1 6-6h2a6 6 0 0 1 6 6M9 20h1M14 20h1M12 12v4M6 6.5 8 4.5M18 6.5 16 4.5',
  projects: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  terminal: 'M3 5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM7 9l3 3-3 3M13 15h4',
  process: 'M9 3v3M15 3v3M9 18v3M15 18v3M3 9h3M3 15h3M18 9h3M18 15h3M6 6h12v12H6zM10 10h4v4h-4z',
  search: 'M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zM20 20l-4-4',
  refresh: 'M20 11A8 8 0 1 0 20 13M20 4v7h-7',
  close: 'M6 6l12 12M18 6L6 18',
  play: 'M8 5v14l11-7z',
  stop: 'M7 7h10v10H7z',
  edit: 'M4 20h4L19 9l-4-4L4 16v4zM13 7l4 4',
  trash: 'M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v5M14 11v5',
  folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  kill: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM9 9l6 6M15 9l-6 6',
  plus: 'M12 5v14M5 12h14',
  info: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM12 8h.01M11 12h1v5h1',
  doc: 'M6 3h9l4 4v14H6zM14 3v5h5M9 13h6M9 17h6',
  bolt: 'M13 2 4 14h6l-1 8 9-12h-6z',
  lock: 'M5 10h14v11H5zM8 10V7a4 4 0 0 1 8 0v3',
  unlock: 'M5 10h14v11H5zM8 10V7a4 4 0 0 1 7.7-1.5',
  // ---- 网络工具箱 ----
  ping: 'M3 12h3l2-6 3 12 3-9 2 3h5',
  route: 'M5 19a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM19 9a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM7 17h3a4 4 0 0 0 4-4v-1a4 4 0 0 1 4-4h1',
  scan: 'M12 2v3M12 19v3M2 12h3M19 12h3M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8z',
  globe: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM3 12h18M12 3a14 14 0 0 1 0 18 14 14 0 0 1 0-18',
  gauge: 'M4 18a8 8 0 1 1 16 0M12 18l4-6',
  nic: 'M3 8a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM7 12h.01M11 12h.01M15 12h.01M9 17v3M15 17v3',
  monitor: 'M3 5h18v11H3zM8 20h8M12 16v4',
  table: 'M3 5h18v14H3zM3 10h18M9 10v9M15 10v9',
  tag: 'M3 12.5V5a2 2 0 0 1 2-2h7.5L21 11.5 12.5 20zM7.5 7.5h.01',
  power: 'M12 4v8M7.6 6.6a7 7 0 1 0 8.8 0',
  calculator: 'M6 3h12a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM8 7h8M8 12h.01M12 12h.01M16 12h.01M8 16h.01M12 16h.01M16 16h.01',
  book: 'M4 5a2 2 0 0 1 2-2h6v18H6a2 2 0 0 1-2-2zM14 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4z',
  copy: 'M9 9h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V10a1 1 0 0 1 1-1zM5 15V4a1 1 0 0 1 1-1h9',
  download: 'M12 4v11M8 11l4 4 4-4M5 20h14',
  chevron: 'M9 6l6 6-6 6',
  clock: 'M12 3a9 9 0 1 1 0 18 9 9 0 0 1 0-18zM12 7v5l3 2',
  alert: 'M12 4l9 16H3zM12 10v4M12 17h.01',
  check: 'M4 12.5l5 5L20 6.5',
}
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path :d="paths[name] || paths.info" />
  </svg>
</template>
