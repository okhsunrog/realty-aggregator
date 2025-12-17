import { createRouter, createWebHistory } from 'vue-router'
import ListingsView from '@/views/ListingsView.vue'
import ListingDetailView from '@/views/ListingDetailView.vue'

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
      component: ListingDetailView,
    },
  ],
})

export default router
