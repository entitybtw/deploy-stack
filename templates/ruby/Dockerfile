FROM ruby:3.3-alpine
WORKDIR /app
COPY Gemfile* ./
RUN bundle install
COPY . .
CMD ["ruby", "app.rb"]
