/** @type {import('next').NextConfig} */
const previewBasePath = process.env.ANYDESIGN_PREVIEW_BASE_PATH || ""

const nextConfig = {
  output: "export",
  basePath: previewBasePath,
  trailingSlash: true,
  images: {
    unoptimized: true,
  },
}

export default nextConfig
