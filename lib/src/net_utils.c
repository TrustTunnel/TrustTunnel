#include <unistd.h>
#include <memory.h>
#include <sys/socket.h>
#include <netinet/in.h>
#ifdef __linux__
#include <linux/icmp.h>
#endif
#include <netinet/icmp6.h>


/**
 * @return Same as `setsockopt`
 */
extern int set_icmp_filter(int fd);
/**
 * @return Same as `setsockopt`
 */
extern int set_icmpv6_filter(int fd);
/**
 * @return Same as `setsockopt`
 */
extern int bind_to_interface_by_index(int fd, int family, unsigned idx);


int set_icmp_filter(int fd) {
#ifdef __linux__
    struct icmp_filter filter = {};
    filter.data = ~(
            (1 << ICMP_PARAMETERPROB)
            | (1 << ICMP_DEST_UNREACH)
            | (1 << ICMP_TIME_EXCEEDED)
            | (1 << ICMP_ECHOREPLY)
    );
    return setsockopt(fd, SOL_RAW, ICMP_FILTER, &filter, sizeof(filter));
#else
    (void)fd;
    return 0;
#endif
}

int set_icmpv6_filter(int fd) {
    struct icmp6_filter filter;
    ICMP6_FILTER_SETBLOCKALL(&filter);
    ICMP6_FILTER_SETPASS(ICMP6_DST_UNREACH, &filter);
    ICMP6_FILTER_SETPASS(ICMP6_TIME_EXCEEDED, &filter);
    ICMP6_FILTER_SETPASS(ICMP6_PACKET_TOO_BIG, &filter);
    ICMP6_FILTER_SETPASS(ICMP6_PARAM_PROB, &filter);
    ICMP6_FILTER_SETPASS(ICMP6_ECHO_REPLY, &filter);
    return setsockopt(fd, IPPROTO_ICMPV6, ICMP6_FILTER, &filter, sizeof(filter));
}

int bind_to_interface_by_index(int fd, int family, unsigned idx) {
#ifdef __APPLE__
    int level = IPPROTO_IP;
    int option = IP_BOUND_IF;
    if (family == AF_INET6) {
        level = IPPROTO_IPV6;
        option = IPV6_BOUND_IF;
    }

    return setsockopt(fd, level, option, &idx, sizeof(idx));
#else
    /* Linux and FreeBSD don't have IP_BOUND_IF */
    (void)fd;
    (void)family;
    (void)idx;
    return -1;
#endif
}
