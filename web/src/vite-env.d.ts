/// <reference types="vite/client" />

declare module '*.mun' {
  const view: any
  export default view
  export const AuthView: any
  export const Sidebar: any
  export const TopBar: any
  export const Chat: any
  export const FilesPage: any
  export const DiffPage: any
}
