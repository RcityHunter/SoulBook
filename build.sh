#!/bin/bash

# SoulBook 构建脚本
# 支持构建安装版本和生产版本

set -e

BUILD_TYPE="${1:-production}"

echo "🚀 开始构建 SoulBook ($BUILD_TYPE 版本)..."

case "$BUILD_TYPE" in
    "installer")
        echo "📦 构建安装版本 (包含安装向导)..."
        cargo build --features installer --release
        echo "✅ 安装版本构建完成"
        echo "💡 使用方式: ./target/release/soulbook"
        echo "   首次运行时会显示安装向导"
        ;;
    "production")
        echo "📦 构建生产版本..."
        cargo build --release
        echo "✅ 生产版本构建完成"
        echo "💡 使用方式: ./target/release/soulbook"
        echo "   需要预先配置好config/production.toml"
        ;;
    "dev-installer")
        echo "🔧 构建开发版本 (包含安装向导)..."
        cargo build --features installer
        echo "✅ 开发安装版本构建完成"
        echo "💡 使用方式: ./target/debug/soulbook"
        ;;
    "dev")
        echo "🔧 构建开发版本..."
        cargo build
        echo "✅ 开发版本构建完成"
        echo "💡 使用方式: ./target/debug/soulbook"
        ;;
    *)
        echo "❌ 未知的构建类型: $BUILD_TYPE"
        echo "可用选项: installer, production, dev-installer, dev"
        exit 1
        ;;
esac

echo ""
echo "🎉 构建完成!"
