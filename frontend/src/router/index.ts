import { createRouter, createWebHistory } from 'vue-router'
import ListingsView from '@/views/ListingsView.vue'

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    {
      path: '/',
      name: 'listings',
      component: ListingsView,
    },
    {
      path: '/listing/:id',
      name: 'listing-detail',
      component: () => import('@/views/ListingDetailView.vue'),
    },
  ],
})

export default router
