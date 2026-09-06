FROM php:8.2-fpm-alpine
RUN apk add --no-cache sqlite curl-dev oniguruma-dev
RUN docker-php-ext-install curl mbstring opcache
RUN printf '[www]\nuser = www-data\ngroup = www-data\nlisten = 0.0.0.0:9000\npm = dynamic\npm.max_children = 3\npm.start_servers = 1\npm.min_spare_servers = 1\npm.max_spare_servers = 2\n' > /usr/local/etc/php-fpm.d/zz-custom.conf
WORKDIR /var/www
