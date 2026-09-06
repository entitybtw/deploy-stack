FROM php:8.2-fpm-alpine

ARG PHP_EXTENSIONS="curl mbstring opcache"
ARG PHP_MEMORY="256M"
ARG FPM_MAX_CHILDREN=3

RUN apk add --no-cache sqlite curl-dev oniguruma-dev

RUN docker-php-ext-install $PHP_EXTENSIONS

RUN echo "memory_limit=$PHP_MEMORY" > /usr/local/etc/php/conf.d/a.ini && \
    echo "max_execution_time=60" >> /usr/local/etc/php/conf.d/a.ini && \
    echo "session.use_strict_mode=1" >> /usr/local/etc/php/conf.d/a.ini && \
    echo "opcache.memory_consumption=64" >> /usr/local/etc/php/conf.d/a.ini && \
    echo "opcache.max_accelerated_files=2000" >> /usr/local/etc/php/conf.d/a.ini && \
    echo "opcache.revalidate_freq=2" >> /usr/local/etc/php/conf.d/a.ini

RUN printf '[www]\\nuser = www-data\\ngroup = www-data\\nlisten = 0.0.0.0:9000\\npm = dynamic\\npm.max_children = %s\\npm.start_servers = 1\\npm.min_spare_servers = 1\\npm.max_spare_servers = 2\\npm.max_requests = 500\\n' "$FPM_MAX_CHILDREN" \
    > /usr/local/etc/php-fpm.d/zz-custom.conf

WORKDIR /var/www
