FROM php:8.2-fpm-alpine

RUN apk add --no-cache nginx sqlite-dev curl-dev oniguruma-dev

# pdo_sqlite нужно собрать; mbstring и opcache уже включены в образ
RUN docker-php-ext-install pdo_sqlite || true

RUN printf '%s\n' \
  'server {' \
  '    listen 80;' \
  '    server_name _;' \
  '    root /var/www/html;' \
  '    index index.php index.html;' \
  '    client_max_body_size 512M;' \
  '' \
  '    location ~ /\.(?!well-known) { deny all; }' \
  '    location ~* \.(db|sqlite|sqlite3|env|log|bak|old|orig|swp|save|tmp|sql)(-wal|-shm|-journal)?$ { deny all; }' \
  '    location ~ /(install|panel/data)/ { deny all; }' \
  '    location / { try_files $uri $uri/ /index.php?$query_string; }' \
  '    location ~ \.php(/|$) {' \
  '        fastcgi_split_path_info ^(.+\.php)(/.+)$;' \
  '        try_files $fastcgi_script_name =404;' \
  '        fastcgi_pass 127.0.0.1:9000;' \
  '        fastcgi_index index.php;' \
  '        include fastcgi_params;' \
  '        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;' \
  '        fastcgi_param PATH_INFO $fastcgi_path_info;' \
  '        fastcgi_buffering off;' \
  '    }' \
  '}' \
  > /etc/nginx/http.d/default.conf

RUN printf '%s\n' \
  '[www]' \
  'user = www-data' \
  'group = www-data' \
  'listen = 127.0.0.1:9000' \
  'pm = dynamic' \
  'pm.max_children = 10' \
  'pm.start_servers = 2' \
  'pm.min_spare_servers = 1' \
  'pm.max_spare_servers = 4' \
  > /usr/local/etc/php-fpm.d/zz-deploy.conf

WORKDIR /var/www/html
CMD ["sh", "-c", "php-fpm -D && nginx -g 'daemon off;'"]
