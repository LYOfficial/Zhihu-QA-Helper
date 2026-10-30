/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,
  distDir: ".build",
  reactStrictMode: true,
  eslint: { ignoreDuringBuilds: true },
  // 开发时前端在 3000 而资源走 127.0.0.1，Next 15 要求显式声明允许的来源
  allowedDevOrigins: ["127.0.0.1", "localhost"],
};

export default nextConfig;
